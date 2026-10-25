use crate::{
    api::ContainerApi,
    constants,
    model::types::{AnyError, ContainerResult, OneShotResult, RunMode, RunSpec},
    util::{
        backend::ContainerEngine,
        id,
        labels::{self, KeyValue, Labels},
    },
};
use base64::{Engine as _, engine::general_purpose};

use bollard::{
    body_full,
    errors::Error::{self, DockerResponseServerError},
    models::{
        ContainerCreateBody, ContainerCreateResponse, ContainerInspectResponse, ContainerState,
        ContainerStateStatusEnum, ContainerSummary, EndpointSettings, HostConfig, Mount,
        NetworkConnectRequest, PortBinding,
    },
    query_parameters::{
        CreateContainerOptions, InspectContainerOptions, KillContainerOptions,
        ListContainersOptions, LogsOptions, RemoveContainerOptions, StartContainerOptions,
        StopContainerOptions, UploadToContainerOptionsBuilder,
    },
};

use bollard_stubs::models::{DeviceMapping, MountType, NetworkingConfig};
use bollard_stubs::query_parameters::{WaitContainerOptions, WaitContainerOptionsBuilder};
use futures::{StreamExt, TryStreamExt, future};
use std::{collections::HashMap, time::Duration};
use tokio::time::{sleep, timeout};

const INJECT_PREFIX: &str = "echo '";
const INJECT_SUFFIX: &str = "' | base64 -d";

pub fn inject(script: &str, name: &str) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "{}{}{} > /tmp/{} && chmod +x /tmp/{} && /tmp/{}",
            INJECT_PREFIX,
            general_purpose::STANDARD.encode(script.trim()),
            INJECT_SUFFIX,
            name,
            name,
            name,
        ),
    ]
}

// Injected scripts are rendered from configuration that may reference the operator's
// secrets, so their payload must not reach a log - base64 is an encoding, not a guard.
// The surrounding command still says which script runs and where it lands, which is all
// the diagnostics need.
pub fn redact_injected(cmd: &[&str]) -> Vec<String> {
    cmd.iter()
        .map(
            |arg| match (arg.find(INJECT_PREFIX), arg.rfind(INJECT_SUFFIX)) {
                (Some(start), Some(end)) if start + INJECT_PREFIX.len() <= end => format!(
                    "{}<redacted>{}",
                    &arg[..start + INJECT_PREFIX.len()],
                    &arg[end..]
                ),
                _ => arg.to_string(),
            },
        )
        .collect()
}

impl<'a> ContainerApi<'a> {
    pub async fn get_all(&self, labels: &Labels) -> Result<Vec<ContainerSummary>, AnyError> {
        let list_options = ListContainersOptions {
            filters: Some(labels.clone().into()),
            all: true,
            ..Default::default()
        };

        Ok(self.client.list_containers(Some(list_options)).await?)
    }

    pub async fn get_running(&self, labels: &Labels) -> Result<Vec<ContainerSummary>, AnyError> {
        let list_options = ListContainersOptions {
            filters: Some(labels.clone().into()),
            all: false,
            ..Default::default()
        };

        Ok(self.client.list_containers(Some(list_options)).await?)
    }

    // The labels of an existing container, or None when there is no such container. A container
    // rooz created carries what rooz knew at creation time, which is how the next run finds out
    // what it built - a name on its own says nothing about who created it.
    pub async fn labels_of(
        &self,
        container_name: &str,
    ) -> Result<Option<HashMap<String, String>>, AnyError> {
        match self
            .client
            .inspect_container(container_name, None::<InspectContainerOptions>)
            .await
        {
            Ok(response) => Ok(Some(
                response
                    .config
                    .and_then(|c| c.labels)
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            )),
            Err(DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn get_single(&self, labels: &Labels) -> Result<Option<ContainerSummary>, AnyError> {
        match self.get_all(labels).await?.as_slice() {
            [] => Ok(None),
            [container] => Ok(Some(container.clone())),
            // as with volumes: engine-writable labels must not be able to abort rooz
            containers => Err(format!(
                "Expected one container matching {:?}, found {}. Remove the ones that are not \
                 rooz's and try again.",
                HashMap::<String, String>::from(labels.clone()),
                containers.len()
            )
            .into()),
        }
    }

    pub async fn remove(&self, container_id: &str, force: bool) -> Result<(), AnyError> {
        let force_display = if force { " (force)" } else { "" };

        if force {
            match self
                .client
                .inspect_container(container_id, None::<InspectContainerOptions>)
                .await
            {
                Ok(ContainerInspectResponse { state, .. }) => {
                    if let Some(ContainerState {
                        status: Some(ContainerStateStatusEnum::RUNNING),
                        ..
                    }) = state
                    {
                        self.kill(container_id, true).await?;
                    }
                }
                Err(Error::JsonDataError { message, .. }) => {
                    if message.starts_with("unknown variant `stopped`") {
                        // hack: https://github.com/containers/podman/issues/17728
                        // nothing to kill as the container is already stopped
                    } else {
                        return Err(message.into());
                    }
                }
                Err(DockerResponseServerError {
                    status_code: 404,
                    message,
                }) => {
                    log::debug!("Container no longer exists: {}", message);
                }
                Err(e) => return Err(e.into()),
            }
        }

        match self
            .client
            .remove_container(
                container_id,
                Some(RemoveContainerOptions {
                    force,
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(_) => {
                log::debug!("Removed container: {}{}", container_id, force_display);
                self.wait_removed(container_id).await;
                Ok(())
            }
            Err(DockerResponseServerError {
                status_code: 404, ..
            }) => {
                log::debug!(
                    "No such container. Skipping: {}{}",
                    container_id,
                    force_display
                );
                Ok(())
            }
            Err(DockerResponseServerError {
                status_code: 409, ..
            }) => {
                log::debug!(
                    "Container removal already in progress. Skipping: {}{}",
                    container_id,
                    force_display
                );
                self.wait_removed(container_id).await;
                Ok(())
            }
            Err(DockerResponseServerError {
                status_code,
                message,
            }) => Err(format!(
                "{} (Error code: {})",
                message.replace("\"", ""),
                status_code
            )
            .into()),
            Err(e) => Err(e.into()),
        }
    }

    async fn wait_removed(&self, container_id: &str) {
        let _ = timeout(Duration::from_secs(30), async {
            while self
                .client
                .inspect_container(container_id, None::<InspectContainerOptions>)
                .await
                .is_ok()
            {
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
    }

    pub async fn kill(&self, container_id: &str, wait_for_remove: bool) -> Result<(), AnyError> {
        match self
            .client
            .kill_container(container_id, None::<KillContainerOptions>)
            .await
        {
            Ok(_) => {
                if wait_for_remove {
                    timeout(Duration::from_secs(5), async {
                        loop {
                            match self
                                .client
                                .inspect_container(container_id, None::<InspectContainerOptions>)
                                .await
                            {
                                Ok(ContainerInspectResponse { state, .. }) => {
                                    if let Some(ContainerState {
                                        status: Some(ContainerStateStatusEnum::EXITED),
                                        ..
                                    }) = state
                                    {
                                        return Ok(());
                                    } else {
                                        sleep(Duration::from_millis(100)).await
                                    }
                                }
                                Err(Error::JsonDataError { message, .. }) => {
                                    if message.starts_with("unknown variant `stopped`") {
                                        // hack: https://github.com/containers/podman/issues/17728
                                        // nothing to kill as the container is already stopped
                                        return Ok(());
                                    } else {
                                        return Err(message.into());
                                    }
                                }
                                //Podman backend
                                Err(DockerResponseServerError {
                                    status_code: 500,
                                    message,
                                }) if message.ends_with("no such container") => return Ok(()),
                                //Docker backend
                                Err(DockerResponseServerError {
                                    status_code: 404, ..
                                }) => return Ok(()),
                                Err(e) => return Err(e.into()),
                            }
                        }
                    })
                    .await?
                } else {
                    sleep(Duration::from_millis(10)).await;
                    Ok(())
                }
            }
            Err(e) => Err(Box::new(e)),
        }
    }

    pub async fn stop(&self, container_id: &str) -> Result<(), AnyError> {
        self.client
            .stop_container(
                container_id,
                Some(StopContainerOptions {
                    t: Some(0),
                    signal: Some("SIGINT".into()),
                }),
            )
            .await?;
        let mut count = 10;
        while count > 0 {
            log::debug!("Waiting for container {} to be gone...", container_id);
            let r = self
                .client
                .inspect_container(container_id, None::<InspectContainerOptions>)
                .await;
            if let Err(DockerResponseServerError {
                status_code: 404, ..
            }) = r
            {
                break;
            } else {
                sleep(Duration::from_millis(100)).await;
                count -= 1;
            }
        }

        Ok(())
    }

    async fn create_core(&self, spec: RunSpec<'a>) -> Result<ContainerCreateResponse, AnyError> {
        let image_info = self.image.ensure(spec.image, spec.force_pull).await?;

        let options = CreateContainerOptions {
            name: Some(spec.container_name.to_string()),
            platform: image_info.platform.unwrap(),
        };

        let oom_score_adj = match self.backend.engine {
            ContainerEngine::Podman => Some(100),
            _ => None,
        };

        let localhost = "127.0.0.1";
        let port_bindings = spec.ports.clone().map(|ports| {
            let mut bindings = HashMap::<String, Option<Vec<PortBinding>>>::new();

            for (source, target) in &ports {
                bindings.insert(
                    if source.contains('/') {
                        source.to_string()
                    } else {
                        format!("{}/tcp", source)
                    },
                    Some(vec![PortBinding {
                        host_port: target.as_deref().map(|x| x.to_string()),
                        host_ip: Some(localhost.to_string()),
                    }]),
                );
            }

            bindings
        });

        let mut labels = spec.labels.clone();
        if let Some(ports) = &spec.ports {
            let forward_ports: Vec<String> = ports.keys().map(|p| p.to_string()).collect();

            if !forward_ports.is_empty() {
                let all_ports = forward_ports.join(",");
                labels.append((labels::FORWARD_PORTS, &all_ports));
            }
        }

        let (attach_stdin, tty, open_stdin, auto_remove, readonly_rootfs) = {
            // TODO: Podman is less restrictive to what can be done with read-only rootfs via API
            // Docker is much more demanding and the current impl of rooz breaks severely
            // Rancher - untested...
            let readonly_rootfs = match self.backend.engine {
                ContainerEngine::Podman => Some(true),
                _ => None,
            };

            match spec.run_mode {
                RunMode::Workspace => (Some(true), Some(true), None, None, None),
                RunMode::Tmp => (Some(true), Some(true), None, Some(true), None),
                RunMode::Git => (None, None, Some(true), Some(true), readonly_rootfs),
                RunMode::OneShot => (None, None, None, Some(true), readonly_rootfs),
                RunMode::Sidecar => (None, None, None, None, readonly_rootfs),
                RunMode::SidecarInstall => (None, None, Some(true), None, None),
            }
        };

        let (cap_add, devices, sysctls) = if spec.vpn {
            (
                Some(vec!["NET_ADMIN".to_string()]),
                Some(vec![DeviceMapping {
                    path_on_host: Some("/dev/net/tun".to_string()),
                    path_in_container: Some("/dev/net/tun".to_string()),
                    cgroup_permissions: Some("rwm".to_string()),
                }]),
                Some(HashMap::from([(
                    "net.ipv4.conf.all.src_valid_mark".to_string(),
                    "1".to_string(),
                )])),
            )
        } else {
            (None, None, None)
        };

        let host_config = HostConfig {
            auto_remove,
            mounts: spec.mounts,
            restart_policy: None,
            oom_score_adj,
            privileged: Some(spec.privileged),
            cap_add,
            devices,
            sysctls,
            init: Some(spec.init),
            readonly_rootfs,
            port_bindings,
            network_mode: spec.default_network.map(|x| x.to_string()),
            ..Default::default()
        };

        let mut env_kv = vec![
            KeyValue::new("ROOZ_META_IMAGE", spec.image),
            KeyValue::new("ROOZ_META_UID", spec.uid),
            KeyValue::new("ROOZ_META_USER", spec.user),
            KeyValue::new("ROOZ_META_HOME", spec.home_dir),
            KeyValue::new("ROOZ_META_WORKSPACE", spec.workspace_key),
            KeyValue::new("ROOZ_META_CONTAINER_NAME", spec.container_name),
        ];

        // podman fixes HOME when the container is created - before rooz adds its user, so it falls
        // back to the working directory - where docker looks it up for each exec: set, it is the
        // same on both
        if !spec.home_dir.is_empty() && !spec.env.as_ref().is_some_and(|e| e.contains_key("HOME")) {
            env_kv.push(KeyValue::new("HOME", spec.home_dir));
        }
        if let Some(env) = spec.env {
            env_kv.extend(KeyValue::to_vec(env));
        }

        let env = KeyValue::to_vec_str(&env_kv);

        let config = ContainerCreateBody {
            image: Some(spec.image.to_string()),
            entrypoint: spec
                .command
                .map(|vec| vec.iter().map(|&s| s.to_string()).collect()),
            cmd: spec
                .args
                .map(|vec| vec.iter().map(|&s| s.to_string()).collect()),
            working_dir: spec.work_dir.map(|s| s.to_string()),
            // THIS MUST BE spec.uid, NOT spec.user - otherwise file ownership will break
            user: Some(spec.uid.to_string()),
            attach_stdin,
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            tty,
            open_stdin,
            host_config: Some(host_config),
            labels: Some(labels.into()),
            env: Some(env.iter().map(|&s| s.to_string()).collect()),
            networking_config: spec.default_network.map(|n| NetworkingConfig {
                endpoints_config: Some(HashMap::from([(
                    n.to_string(),
                    EndpointSettings {
                        aliases: spec.network_aliases.clone(),
                        ..Default::default()
                    },
                )])),
            }),
            ..Default::default()
        };

        let response = self
            .client
            .create_container(Some(options.clone()), config.clone())
            .await
            .map_err(|e| {
                format!(
                    "Could not create container '{}': {}",
                    spec.container_name, e
                )
            })?;

        if let Some(networks) = &spec.additional_networks {
            for n in networks {
                if let Err(e) = self
                    .client
                    .connect_network(
                        n,
                        NetworkConnectRequest {
                            container: response.id.to_string(),
                            endpoint_config: Some(EndpointSettings {
                                aliases: spec.network_aliases.clone(),
                                ..Default::default()
                            }),
                        },
                    )
                    .await
                {
                    // a half-networked container would be adopted as-is by the next run
                    self.client
                        .remove_container(
                            &response.id,
                            Some(RemoveContainerOptions {
                                force: true,
                                ..Default::default()
                            }),
                        )
                        .await
                        .ok();
                    return Err(format!(
                        "Could not connect container '{}' to network '{}': {}",
                        spec.container_name, n, e
                    )
                    .into());
                }
            }
        }
        log::debug!(
            "Created container: {} ({})",
            spec.container_name,
            response.id
        );
        Ok(response)
    }

    pub async fn create(&self, spec: RunSpec<'a>) -> Result<ContainerResult, AnyError> {
        log::debug!(
            "[{}: {:?}]: CREATE CONTAINER - name: {}, uid: {}, user: {}, image: {}, entrypoint: {}",
            spec.reason,
            spec.run_mode,
            spec.container_name,
            spec.uid,
            spec.user,
            spec.image,
            spec.command.clone().unwrap_or(vec![]).join(" ")
        );

        let existing = self
            .client
            .inspect_container(spec.container_name, None::<InspectContainerOptions>)
            .await;
        if let Ok(found) = &existing {
            let found = found
                .config
                .as_ref()
                .and_then(|c| c.labels.clone())
                .unwrap_or_default();
            if !same_owner(&spec.labels, &found) {
                return Err(format!(
                    "A container named '{}' already exists and is not this workspace's: refusing to \
                     reuse or replace it. Remove or rename it if it is a leftover.",
                    spec.container_name
                )
                .into());
            }
        }
        let container_id = match existing {
            Ok(ContainerInspectResponse { id: Some(id), .. }) if !spec.force_recreate => {
                ContainerResult::AlreadyExists { id }
            }
            Ok(ContainerInspectResponse { id: Some(id), .. }) => {
                let remove_options = RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                };

                self.client
                    .remove_container(&id, Some(remove_options))
                    .await?;
                let response = self.create_core(spec).await?;

                ContainerResult::Created { id: response.id }
            }
            Ok(ContainerInspectResponse { id: None, .. }) => unreachable!(),
            Err(DockerResponseServerError {
                status_code: 404, ..
            }) => {
                let response = self.create_core(spec).await?;

                ContainerResult::Created { id: response.id }
            }
            Err(err) => {
                return Err(format!(
                    "Could not inspect container '{}': {}",
                    spec.container_name, err
                )
                .into());
            }
        };
        Ok(container_id.clone())
    }

    pub async fn start(&self, container_id: &str) -> Result<(), Error> {
        self.client
            .start_container(container_id, None::<StartContainerOptions>)
            .await
    }

    async fn make_one_shot(
        &self,
        name: &str,
        mounts: Option<Vec<Mount>>,
        uid: Option<&str>,
        image: Option<&str>,
    ) -> Result<String, AnyError> {
        let wait_for_exec = r#"#!/bin/sh
TIMEOUT=${EXEC_TIMEOUT:-30}
mkfifo /tmp/exec_start /tmp/exec_end

echo "Waiting for exec session (timeout: ${TIMEOUT}s)..."
timeout $TIMEOUT sh -c 'read _ < /tmp/exec_start' || exit 1

echo "Exec session started"
read _ < /tmp/exec_end
EXEC_EXIT_CODE=$(cat /tmp/exec_exit)
echo "Exec session ended: $EXEC_EXIT_CODE"
exit $EXEC_EXIT_CODE"#;

        let epv = inject(wait_for_exec, "entrypoint.sh");
        let entrypoint = epv.iter().map(String::as_str).collect();
        let work_dir = "/tmp/one-shot";
        let id = self
            .create(RunSpec {
                reason: name,
                image: image.unwrap_or(constants::HELPER_IMAGE),
                container_name: &id::random_suffix("one-shot"),
                command: Some(entrypoint),
                mounts: mounts.map(|mut m| {
                    m.extend_from_slice(&[Mount {
                        target: Some(work_dir.into()),
                        typ: Some(MountType::TMPFS),
                        ..Default::default()
                    }]);
                    m
                }),
                uid: uid.unwrap_or(constants::ROOT_UID),
                work_dir: Some(work_dir),
                ..Default::default()
            })
            .await
            .map(|r| r.id().to_string())?;

        self.start(&id).await?;

        if log::log_enabled!(log::Level::Debug) {
            let docker_logs = self.client.clone();
            let s_id = id.clone();
            let s_name = name.to_string();

            tokio::spawn(async move {
                let logs_stream = docker_logs.logs(
                    &s_id,
                    Some(LogsOptions {
                        follow: true,
                        stdout: true,
                        stderr: true,
                        ..Default::default()
                    }),
                );

                logs_stream
                    .for_each(|x| {
                        match x {
                            Ok(r) => log::debug!(
                                "{} | {}",
                                s_name,
                                String::from_utf8_lossy(r.into_bytes().as_ref())
                                    .to_string()
                                    .trim_end()
                            ),
                            Err(err) => log::debug!("{} | log stream ended: {}", s_name, err),
                        };
                        future::ready(())
                    })
                    .await;
            });
        }
        Ok(id)
    }

    fn format_cmd(command: String) -> Vec<String> {
        let cmd = format!(
            r#"#!/bin/sh
trap 'echo $? > /tmp/exec_exit; echo end > /tmp/exec_end' EXIT
echo start > /tmp/exec_start
{}
        "#,
            command
        );
        inject(&cmd, "exec.sh")
    }

    pub async fn one_shot_output(
        &self,
        name: &str,
        command: String,
        mounts: Option<Vec<Mount>>,
        uid: Option<&str>,
        image: Option<&str>,
    ) -> Result<OneShotResult, AnyError> {
        let id = self.make_one_shot(name, mounts, uid, image).await?;
        let cmd = Self::format_cmd(command);
        let cmd = cmd.iter().map(|x| x.as_str()).collect::<Vec<_>>();

        let id_clone = id.clone();
        let client = self.client.clone();

        let wait_handle = tokio::spawn(async move {
            client
                .wait_container(&id_clone, None::<WaitContainerOptions>)
                .try_collect::<Vec<_>>()
                .await
        });

        let data = self.exec.output(name, &id, uid, Some(cmd)).await?;
        let _ = match wait_handle.await? {
            Ok(r) => r,
            Err(Error::DockerContainerWaitError { code, .. }) => {
                return Err(format!("One-shot cmd failed (exit {}): \n{}", code, data).into());
            }
            Err(e) => return Err(e.into()),
        };

        // auto-removal follows the exit by a moment: until it is done the container still holds
        // its mounts, and a caller that removes one of those volumes next would find it in use
        let _ = timeout(Duration::from_secs(5), async {
            while self
                .client
                .inspect_container(&id, None::<InspectContainerOptions>)
                .await
                .is_ok()
            {
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await;

        Ok(OneShotResult { data })
    }

    pub async fn one_shot(
        &self,
        name: &str,
        command: String,
        mounts: Option<Vec<Mount>>,
        uid: Option<&str>,
        image: Option<&str>,
    ) -> Result<(), AnyError> {
        let id = self.make_one_shot(name, mounts, uid, image).await?;
        let cmd = Self::format_cmd(command);
        let cmd = cmd.iter().map(|x| x.as_str()).collect::<Vec<_>>();

        // The container exits with the command's code and is removed with it, and podman removes
        // the command's exec session along with it: the code is read from the container.
        let id_clone = id.clone();
        let client = self.client.clone();
        let wait_handle = tokio::spawn(async move {
            client
                .wait_container(&id_clone, None::<WaitContainerOptions>)
                .try_collect::<Vec<_>>()
                .await
        });
        self.exec.stream(name, &id, uid, Some(cmd)).await?;
        match wait_handle.await? {
            Ok(_) => Ok(()),
            Err(Error::DockerContainerWaitError { code, .. }) => {
                Err(format!("{}: exec failed with exit code {}", name, code).into())
            }
            Err(e) => Err(e.into()),
        }
    }

    pub async fn upload(
        &self,
        container_id: &str,
        dest_path: &str,
        tar: Vec<u8>,
    ) -> Result<(), AnyError> {
        let options = UploadToContainerOptionsBuilder::default()
            .path(dest_path)
            .build();
        self.client
            .upload_to_container(container_id, Some(options), body_full(tar.into()))
            .await?;
        Ok(())
    }

    // A created, never started container with one volume mounted - enough to read the volume
    // through the archive API on any engine, and to write it.
    pub async fn volume_archive_container(
        &self,
        volume: &str,
        target: &str,
        read_only: bool,
    ) -> Result<String, AnyError> {
        self.create(RunSpec {
            reason: "move-copy",
            image: constants::HELPER_IMAGE,
            container_name: &id::random_suffix("one-shot"),
            command: Some(vec!["true"]),
            mounts: Some(vec![Mount {
                typ: Some(MountType::VOLUME),
                source: Some(volume.to_string()),
                target: Some(target.to_string()),
                read_only: Some(read_only),
                ..Default::default()
            }]),
            uid: constants::ROOT_UID,
            ..Default::default()
        })
        .await
        .map(|r| r.id().to_string())
    }

    // A running container with one volume mounted read-only at `target`, to tar it from: the archive
    // API's download cannot be used, as podman's reports every file as root's.
    pub async fn volume_tar_container(
        &self,
        volume: &str,
        target: &str,
    ) -> Result<String, AnyError> {
        let id = self
            .create(RunSpec {
                reason: "move-copy",
                image: constants::HELPER_IMAGE,
                container_name: &id::random_suffix("one-shot"),
                command: Some(vec!["sleep"]),
                args: Some(vec!["infinity"]),
                mounts: Some(vec![Mount {
                    typ: Some(MountType::VOLUME),
                    source: Some(volume.to_string()),
                    target: Some(target.to_string()),
                    read_only: Some(true),
                    ..Default::default()
                }]),
                uid: constants::ROOT_UID,
                ..Default::default()
            })
            .await
            .map(|r| r.id().to_string())?;
        if let Err(e) = self.start(&id).await {
            self.remove(&id, true).await.ok();
            return Err(e.into());
        }
        Ok(id)
    }

    pub async fn upload_tar_stream(
        &self,
        container_id: &str,
        dest_path: &str,
        tar: impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send + 'static,
    ) -> Result<(), AnyError> {
        // no copyUIDGID: it hands every file to the container's user, where without it the tar's
        // owners are kept
        let options = UploadToContainerOptionsBuilder::default()
            .path(dest_path)
            .build();
        self.client
            .upload_to_container(container_id, Some(options), bollard::body_try_stream(tar))
            .await?;
        Ok(())
    }

    pub async fn one_shot_upload(
        &self,
        name: &str,
        mounts: Vec<Mount>,
        dest_path: &str,
        tar: Option<Vec<u8>>,
        command: Option<String>,
    ) -> Result<(), AnyError> {
        let cmd = command.unwrap_or("true".to_string());
        let id = self
            .create(RunSpec {
                reason: name,
                image: constants::HELPER_IMAGE,
                container_name: &id::random_suffix("one-shot"),
                command: Some(vec!["sh", "-c", &cmd]),
                mounts: Some(mounts),
                uid: constants::ROOT_UID,
                ..Default::default()
            })
            .await
            .map(|r| r.id().to_string())?;

        let client = self.client.clone();
        let wait_id = id.clone();
        let wait_handle = tokio::spawn(async move {
            let next_exit = WaitContainerOptionsBuilder::new()
                .condition("next-exit")
                .build();
            client
                .wait_container(&wait_id, Some(next_exit))
                .try_collect::<Vec<_>>()
                .await
        });

        let upload_and_start: Result<(), AnyError> = async {
            if let Some(tar) = tar {
                self.upload(&id, dest_path, tar)
                    .await
                    .map_err(|e| format!("{}: uploading files failed: {}", name, e))?;
            }
            Ok(self.start(&id).await?)
        }
        .await;

        if let Err(e) = upload_and_start {
            wait_handle.abort();
            self.remove(&id, true).await.ok();
            return Err(e);
        }

        match wait_handle.await? {
            Ok(_) => Ok(()),
            Err(Error::DockerContainerWaitError { code, .. }) => {
                Err(format!("{}: one-shot command failed (exit {})", name, code).into())
            }
            Err(e) => Err(e.into()),
        }
    }
}

// The workspace and container a name was created for, read back from its labels: a name alone
// says nothing about whose it is.
pub fn same_owner(expected: &Labels, found: &HashMap<String, String>) -> bool {
    let expected: HashMap<String, String> = expected.clone().into();
    [labels::WORKSPACE_KEY, labels::CONTAINER]
        .iter()
        .all(|k| expected.get(*k).is_none_or(|v| found.get(*k) == Some(v)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_name_is_reused_only_for_its_own_workspace_and_container() {
        use super::same_owner;
        use crate::util::labels::{self, Labels};
        use std::collections::HashMap;
        let expected = Labels::from(&[Labels::workspace("a"), Labels::container("x")]);
        let found = |ws: &str, c: &str| {
            HashMap::from([
                (labels::WORKSPACE_KEY.to_string(), ws.to_string()),
                (labels::CONTAINER.to_string(), c.to_string()),
            ])
        };
        assert!(same_owner(&expected, &found("a", "x")));
        assert!(!same_owner(&expected, &found("a-x", "work")));
        assert!(!same_owner(&expected, &HashMap::new()));
        // a name without expectations (a one-shot helper) is not checked
        assert!(same_owner(&Labels::default(), &found("b", "y")));
    }

    use super::{inject, redact_injected};

    #[test]
    fn injected_payloads_never_reach_a_log() {
        let script = "#!/bin/sh\necho \"TOKEN=SUP3R-SECRET\"";
        let cmd = inject(script, "install-0.sh");
        let payload = general_purpose_encode(script);
        assert!(cmd[2].contains(&payload), "test does not exercise inject");

        let redacted = redact_injected(&cmd.iter().map(String::as_str).collect::<Vec<_>>());
        let joined = redacted.join(" ");
        assert!(
            !joined.contains(&payload),
            "payload survived redaction: {}",
            joined
        );
        assert_eq!(
            redacted[2],
            "echo '<redacted>' | base64 -d > /tmp/install-0.sh \
             && chmod +x /tmp/install-0.sh && /tmp/install-0.sh"
        );
    }

    #[test]
    fn plain_commands_are_logged_verbatim() {
        assert_eq!(
            redact_injected(&["chown", "-R", "1000:1000", "'/work'"]),
            vec!["chown", "-R", "1000:1000", "'/work'"]
        );
    }

    fn general_purpose_encode(value: &str) -> String {
        use base64::{Engine, engine::general_purpose};
        general_purpose::STANDARD.encode(value)
    }
}
