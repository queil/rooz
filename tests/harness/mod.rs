// The harness is compiled into every integration test binary, and each of them uses a
// subset of it - so "never used" here means "not used by this binary", not unused.
#![allow(dead_code)]

use assert_cmd::Command;
use bollard::{
    API_DEFAULT_VERSION, Docker,
    exec::{CreateExecOptions, StartExecResults},
    models::{ContainerCreateBody, HostConfig, Mount, VolumeCreateRequest},
    query_parameters::{
        CreateContainerOptions, CreateImageOptions, ListContainersOptions, ListNetworksOptions,
        ListVolumesOptions, LogsOptions, RemoveContainerOptions, RemoveVolumeOptions,
        StartContainerOptions, WaitContainerOptions,
    },
};
use bollard_stubs::models::{ContainerSummary, ContainerSummaryStateEnum, MountType, Volume};
use futures::StreamExt;
use std::{collections::HashMap, env, fs, io::Write};

pub fn write_cfg(key: &str, yaml: &str) -> String {
    let path = format!("/tmp/rooz-test-{}.yaml", key);
    let mut f = fs::File::create(&path).expect("write config");
    f.write_all(yaml.as_bytes()).unwrap();
    path
}

/// The pinned consent value rooz asks for, e.g. `wg@0123456789abcdef`: a dry run without it is
/// refused, and the refusal names `<var>=<value>` as the remedy.
pub fn pinned_consent(env: &TestEnv, key: &str, cfg_path: &str, var: &str) -> String {
    let out = env
        .rooz()
        .args(["new", key, "--config", cfg_path, "--dry-run"])
        .env_remove(var)
        .output()
        .unwrap();
    assert!(!out.status.success(), "no consent was asked for");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let start = stderr
        .find(&format!("{}=", var))
        .unwrap_or_else(|| panic!("no {} remedy in:\n{}", var, stderr))
        + var.len()
        + 1;
    stderr[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || "@,-_".contains(*c))
        .collect()
}

pub fn cleanup(env: &TestEnv, key: &str, cfg_path: &str) {
    env.rooz().args(["rm", key, "--force"]).assert().success();
    let _ = fs::remove_file(cfg_path);
}

pub struct TestEnv {
    pub docker_host: String,
    pub docker: Docker,
}

impl TestEnv {
    // Without a daemon the integration tests return early and pass. Where they are meant to run,
    // ROOZ_TEST_REQUIRE_DAEMON turns that into a failure, so a daemon that did not start shows.
    pub fn from_env() -> Option<Self> {
        let env = Self::connect_from_env();
        if env.is_none() && env::var("ROOZ_TEST_REQUIRE_DAEMON").is_ok() {
            panic!(
                "ROOZ_TEST_REQUIRE_DAEMON is set but there is no daemon: ROOZ_TEST_DOCKER_HOST={:?}, \
                 ROOZ_TEST_ENGINE={:?}",
                env::var("ROOZ_TEST_DOCKER_HOST").ok(),
                env::var("ROOZ_TEST_ENGINE").ok()
            );
        }
        env
    }

    fn connect_from_env() -> Option<Self> {
        let docker_host = env::var("ROOZ_TEST_DOCKER_HOST").ok()?;
        // both variables have to be set for the suite to run against a daemon
        env::var("ROOZ_TEST_ENGINE").ok()?;
        let docker = connect(&docker_host).ok()?;
        Some(Self {
            docker_host,
            docker,
        })
    }

    pub fn rooz(&self) -> Command {
        let mut cmd = Command::cargo_bin("rooz").unwrap();
        // a stuck engine call fails the test, naming the command, instead of hanging the suite
        cmd.timeout(std::time::Duration::from_secs(300))
            .env("DOCKER_HOST", &self.docker_host)
            .env("http_proxy", "")
            .env("HTTP_PROXY", "")
            // the suite runs 'system init --force', which replaces the age identity - keep
            // it away from the developer's own (~/.config/rooz/age.key)
            .env(
                "ROOZ_AGE_KEY_FILE",
                env::temp_dir().join("rooz-test-identity").join("age.key"),
            );
        cmd
    }

    pub async fn containers_by_workspace(&self, key: &str) -> Vec<ContainerSummary> {
        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec![format!("dev.rooz.workspace={}", key)],
        );
        let opts = ListContainersOptions {
            all: true,
            filters: Some(filters),
            ..Default::default()
        };
        self.docker
            .list_containers(Some(opts))
            .await
            .unwrap_or_default()
    }

    // rooz's clone containers carry no workspace label
    pub async fn git_containers(&self) -> Vec<String> {
        let mut filters = HashMap::new();
        filters.insert("label".to_string(), vec!["dev.rooz.role=git".to_string()]);
        let opts = ListContainersOptions {
            all: true,
            filters: Some(filters),
            ..Default::default()
        };
        let mut ids = self
            .docker
            .list_containers(Some(opts))
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| c.id)
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub async fn volumes_by_workspace(&self, key: &str) -> Vec<Volume> {
        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec![format!("dev.rooz.workspace={}", key)],
        );
        let opts = ListVolumesOptions {
            filters: Some(filters),
        };
        self.docker
            .list_volumes(Some(opts))
            .await
            .ok()
            .and_then(|r| r.volumes)
            .unwrap_or_default()
    }

    pub async fn volume_file(&self, volume_name: &str, file_path: &str) -> String {
        self.volume_cmd(
            volume_name,
            vec!["cat".to_string(), format!("/mnt/{}", file_path)],
        )
        .await
    }

    /// Returns "<octal mode> <owner uid>" of a file inside a volume, e.g. "755 1000".
    pub async fn volume_stat(&self, volume_name: &str, file_path: &str) -> String {
        self.volume_cmd(
            volume_name,
            vec![
                "stat".to_string(),
                "-c".to_string(),
                "%a %u".to_string(),
                format!("/mnt/{}", file_path),
            ],
        )
        .await
        .trim()
        .to_string()
    }

    /// Pulls `image` unless the engine has it: creating a container through the API does not.
    pub async fn ensure_image(&self, image: &str) {
        if self.docker.inspect_image(image).await.is_ok() {
            return;
        }
        let mut pull = self.docker.create_image(
            Some(CreateImageOptions {
                from_image: Some(image.to_string()),
                ..Default::default()
            }),
            None,
            None,
        );
        while let Some(progress) = pull.next().await {
            progress.unwrap_or_else(|e| panic!("pulling {}: {}", image, e));
        }
    }

    /// Runs `script` as root in a throwaway `image` container, with `volume` writable at /mnt, and
    /// waits for it to finish.
    pub async fn run_in_volume(&self, image: &str, volume: &str, script: &str) {
        self.ensure_image(image).await;
        let cname = format!("rooz-test-run-{}", volume.replace(['/', ':'], "-"));
        let body = ContainerCreateBody {
            image: Some(image.to_string()),
            entrypoint: Some(vec!["sh".to_string(), "-c".to_string()]),
            cmd: Some(vec![script.to_string()]),
            user: Some("0".to_string()),
            host_config: Some(HostConfig {
                mounts: Some(vec![Mount {
                    target: Some("/mnt".to_string()),
                    source: Some(volume.to_string()),
                    typ: Some(MountType::VOLUME),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(cname.clone()),
                    ..Default::default()
                }),
                body,
            )
            .await
            .unwrap();
        self.docker
            .start_container(&cname, None::<StartContainerOptions>)
            .await
            .unwrap();
        self.docker
            .wait_container(&cname, None::<WaitContainerOptions>)
            .next()
            .await;
        self.remove_decoy_container(&cname).await;
    }

    /// Serves `dir` of `volume` over plain HTTP from a container named `name`; returns its address.
    pub async fn serve_dir(&self, name: &str, volume: &str, dir: &str) -> String {
        let image = "docker.io/library/python:3-alpine";
        self.ensure_image(image).await;
        let body = ContainerCreateBody {
            image: Some(image.to_string()),
            cmd: Some(
                ["python3", "-m", "http.server", "80", "-d", dir]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            host_config: Some(HostConfig {
                mounts: Some(vec![Mount {
                    target: Some("/srv".to_string()),
                    source: Some(volume.to_string()),
                    typ: Some(MountType::VOLUME),
                    read_only: Some(true),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(name.to_string()),
                    ..Default::default()
                }),
                body,
            )
            .await
            .unwrap();
        self.docker
            .start_container(name, None::<StartContainerOptions>)
            .await
            .unwrap();
        let networks = self
            .docker
            .inspect_container(name, None)
            .await
            .unwrap()
            .network_settings
            .and_then(|n| n.networks)
            .unwrap_or_default();
        networks
            .values()
            .find_map(|n| n.ip_address.clone().filter(|ip| !ip.is_empty()))
            .expect("server has no address")
    }

    async fn volume_cmd(&self, volume_name: &str, cmd: Vec<String>) -> String {
        let cname = format!("rooz-test-cat-{}", volume_name.replace(['/', ':'], "-"));
        let mount = Mount {
            target: Some("/mnt".to_string()),
            source: Some(volume_name.to_string()),
            typ: Some(MountType::VOLUME),
            read_only: Some(true),
            ..Default::default()
        };
        self.ensure_image("alpine:latest").await;
        let body = ContainerCreateBody {
            image: Some("alpine:latest".to_string()),
            cmd: Some(cmd),
            host_config: Some(HostConfig {
                mounts: Some(vec![mount]),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(cname.clone()),
                    ..Default::default()
                }),
                body,
            )
            .await
            .unwrap();
        self.docker
            .start_container(&cname, None::<StartContainerOptions>)
            .await
            .unwrap();
        self.docker
            .wait_container(&cname, None::<WaitContainerOptions>)
            .next()
            .await;

        let mut output = String::new();
        let mut logs = self.docker.logs(
            &cname,
            Some(LogsOptions {
                stdout: true,
                stderr: false,
                follow: false,
                ..Default::default()
            }),
        );
        while let Some(Ok(msg)) = logs.next().await {
            output.push_str(&msg.to_string());
        }
        self.docker
            .remove_container(
                &cname,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .ok();
        output
    }

    pub async fn create_decoy_container(&self, name: &str) {
        self.ensure_image("alpine:latest").await;
        let body = ContainerCreateBody {
            image: Some("alpine:latest".to_string()),
            cmd: Some(vec!["true".to_string()]),
            ..Default::default()
        };
        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(name.to_string()),
                    ..Default::default()
                }),
                body,
            )
            .await
            .unwrap();
    }

    pub async fn create_decoy_volume(&self, name: &str) {
        self.docker
            .create_volume(VolumeCreateRequest {
                name: Some(name.to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    pub async fn remove_decoy_container(&self, name: &str) {
        self.docker
            .remove_container(
                name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .ok();
    }

    pub async fn remove_decoy_volume(&self, name: &str) {
        self.docker
            .remove_volume(name, None::<RemoveVolumeOptions>)
            .await
            .ok();
    }

    pub async fn container_exists(&self, name: &str) -> bool {
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), vec![format!("^/{}$", name)]);
        let opts = ListContainersOptions {
            all: true,
            filters: Some(filters),
            ..Default::default()
        };
        !self
            .docker
            .list_containers(Some(opts))
            .await
            .unwrap_or_default()
            .is_empty()
    }

    pub async fn volume_exists(&self, name: &str) -> bool {
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), vec![name.to_string()]);
        let opts = ListVolumesOptions {
            filters: Some(filters),
        };
        self.docker
            .list_volumes(Some(opts))
            .await
            .ok()
            .and_then(|r| r.volumes)
            .map(|vs| !vs.is_empty())
            .unwrap_or(false)
    }

    pub async fn workspace_container_states(&self, key: &str) -> Vec<ContainerSummaryStateEnum> {
        self.containers_by_workspace(key)
            .await
            .into_iter()
            .filter_map(|c| c.state)
            .collect()
    }

    pub async fn exec_code(&self, container: &str, cmd: Vec<&str>) -> i64 {
        self.exec_code_as(container, Some("0"), cmd).await
    }

    // `None` runs it as the container's own user
    pub async fn exec_code_as(&self, container: &str, user: Option<&str>, cmd: Vec<&str>) -> i64 {
        let exec = self
            .docker
            .create_exec(
                container,
                CreateExecOptions {
                    cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
                    user: user.map(str::to_string),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        if let StartExecResults::Attached { mut output, .. } =
            self.docker.start_exec(&exec.id, None).await.unwrap()
        {
            while output.next().await.is_some() {}
        }
        self.docker
            .inspect_exec(&exec.id)
            .await
            .unwrap()
            .exit_code
            .unwrap_or(-1)
    }

    // 0 when `from` opens a TCP connection to port 7 of `to` (the test sidecars listen there). TCP,
    // not ping: podman does not grant the NET_RAW ping needs, so ping proves nothing there.
    pub async fn reach(&self, from: &str, to: &str) -> i64 {
        let probe = format!(
            "for i in 1 2 3; do nc -z -w 2 {} 7 && exit 0; sleep 1; done; exit 1",
            to
        );
        self.exec_code(from, vec!["sh", "-c", &probe]).await
    }

    pub async fn network_names_by_workspace(&self, key: &str) -> Vec<String> {
        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec![format!("dev.rooz.workspace={}", key)],
        );
        let opts = ListNetworksOptions {
            filters: Some(filters),
        };
        let mut names = self
            .docker
            .list_networks(Some(opts))
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|n| n.name)
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    // Plays the co-tenant: builds an image carrying whatever labels it likes and tags it
    // under a name rooz derives from the workspace. Returns the squatted image's id.
    /// Points `repo:tag` at `source`, as a pull or a local build of the same name would.
    pub async fn tag_image(&self, source: &str, repo: &str, tag: &str) {
        self.docker
            .tag_image(
                source,
                Some(bollard_stubs::query_parameters::TagImageOptions {
                    repo: Some(repo.to_string()),
                    tag: Some(tag.to_string()),
                }),
            )
            .await
            .unwrap();
    }

    pub async fn squat_image(&self, repo: &str, labels: &[(&str, &str)], marker: &str) -> String {
        let name = format!("squat-{}", repo.replace(['/', ':', '.'], "-"));
        self.ensure_image("alpine:latest").await;
        let body = ContainerCreateBody {
            image: Some("alpine:latest".to_string()),
            cmd: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("echo squatted > {}", marker),
            ]),
            ..Default::default()
        };
        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(name.clone()),
                    ..Default::default()
                }),
                body,
            )
            .await
            .unwrap();
        self.docker
            .start_container(&name, None::<StartContainerOptions>)
            .await
            .unwrap();
        let mut wait = self
            .docker
            .wait_container(&name, None::<WaitContainerOptions>);
        while wait.next().await.is_some() {}

        let committed = self
            .docker
            .commit_container(
                bollard_stubs::query_parameters::CommitContainerOptions {
                    container: Some(name.clone()),
                    repo: Some(repo.to_string()),
                    tag: Some("latest".to_string()),
                    pause: false,
                    ..Default::default()
                },
                bollard_stubs::models::ContainerConfig {
                    labels: Some(
                        labels
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                            .collect(),
                    ),
                    cmd: Some(vec!["sleep".to_string(), "infinity".to_string()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        self.remove_decoy_container(&name).await;
        committed.id
    }

    pub async fn remove_image(&self, image: &str) {
        self.docker
            .remove_image(
                image,
                Some(bollard_stubs::query_parameters::RemoveImageOptions {
                    force: true,
                    ..Default::default()
                }),
                None,
            )
            .await
            .ok();
    }

    pub async fn all_rooz_volumes(&self) -> Vec<Volume> {
        let mut filters = HashMap::new();
        filters.insert("label".to_string(), vec!["dev.rooz=true".to_string()]);
        let opts = ListVolumesOptions {
            filters: Some(filters),
        };
        self.docker
            .list_volumes(Some(opts))
            .await
            .ok()
            .and_then(|r| r.volumes)
            .unwrap_or_default()
    }
}

fn connect(docker_host: &str) -> Result<Docker, bollard::errors::Error> {
    if docker_host.starts_with("unix://") {
        Docker::connect_with_unix(
            docker_host.trim_start_matches("unix://"),
            120,
            API_DEFAULT_VERSION,
        )
    } else {
        Docker::connect_with_http(docker_host, 120, API_DEFAULT_VERSION)
    }
}

pub fn unique_key(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let pid = std::process::id();
    format!("{}-{}-{}", prefix, pid, ns)
}
