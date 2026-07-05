use assert_cmd::Command;
use bollard::{
    API_DEFAULT_VERSION, Docker,
    models::{ContainerCreateBody, HostConfig, Mount},
    query_parameters::{
        CreateContainerOptions, ListContainersOptions, ListVolumesOptions, LogsOptions,
        RemoveContainerOptions, StartContainerOptions, WaitContainerOptions,
    },
};
use bollard_stubs::models::{ContainerSummary, MountType, Volume};
use futures::StreamExt;
use std::{collections::HashMap, env};

pub struct TestEnv {
    pub docker_host: String,
    pub engine: String,
    pub docker: Docker,
}

impl TestEnv {
    pub fn from_env() -> Option<Self> {
        let docker_host = env::var("ROOZ_TEST_DOCKER_HOST").ok()?;
        let engine = env::var("ROOZ_TEST_ENGINE").ok()?;
        let docker = connect(&docker_host).ok()?;
        Some(Self { docker_host, engine, docker })
    }

    pub fn rooz(&self) -> Command {
        let mut cmd = Command::cargo_bin("rooz").unwrap();
        cmd.env("DOCKER_HOST", &self.docker_host)
            .env("http_proxy", "")
            .env("HTTP_PROXY", "");
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
        self.docker.list_containers(Some(opts)).await.unwrap_or_default()
    }

    pub async fn volumes_by_workspace(&self, key: &str) -> Vec<Volume> {
        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec![format!("dev.rooz.workspace={}", key)],
        );
        let opts = ListVolumesOptions {
            filters: Some(filters),
            ..Default::default()
        };
        self.docker
            .list_volumes(Some(opts))
            .await
            .ok()
            .and_then(|r| r.volumes)
            .unwrap_or_default()
    }

    pub async fn volume_file(&self, volume_name: &str, file_path: &str) -> String {
        let cname = format!(
            "rooz-test-cat-{}",
            volume_name.replace(['/', ':'], "-")
        );
        let mount = Mount {
            target: Some("/mnt".to_string()),
            source: Some(volume_name.to_string()),
            typ: Some(MountType::VOLUME),
            read_only: Some(true),
            ..Default::default()
        };
        let body = ContainerCreateBody {
            image: Some("alpine:latest".to_string()),
            cmd: Some(vec!["cat".to_string(), format!("/mnt/{}", file_path)]),
            host_config: Some(HostConfig {
                mounts: Some(vec![mount]),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.docker
            .create_container(
                Some(CreateContainerOptions { name: Some(cname.clone()), ..Default::default() }),
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
            Some(LogsOptions { stdout: true, stderr: false, follow: false, ..Default::default() }),
        );
        while let Some(Ok(msg)) = logs.next().await {
            output.push_str(&msg.to_string());
        }
        self.docker
            .remove_container(
                &cname,
                Some(RemoveContainerOptions { force: true, ..Default::default() }),
            )
            .await
            .ok();
        output
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
