use std::{
    io::{stdout, Write},
    time::Duration,
};

use base64::{engine::general_purpose, Engine as _};
use bollard::{
    container::{
        Config, CreateContainerOptions, ListContainersOptions, LogOutput::Console, LogsOptions,
        RemoveContainerOptions, StartContainerOptions, StopContainerOptions,
    },
    errors::Error,
    models::HostConfig,
    network::ConnectNetworkOptions,
    service::{ContainerInspectResponse, ContainerSummary, EndpointSettings},
};
use futures::StreamExt;
use tokio::time::sleep;

use crate::{
    backend::{Api, ContainerClient},
    labels::{KeyValue, Labels},
    types::{ContainerResult, RunSpec},
};

pub fn inject(script: &str, name: &str) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "echo '{}' | base64 -d > /tmp/{} && chmod +x /tmp/{} && /tmp/{}",
            general_purpose::STANDARD.encode(script.trim()),
            name,
            name,
            name
        ),
    ]
}

impl<'a> Api<'a> {
    pub async fn get_all_containers(
        &self,
        labels: Labels,
    ) -> Result<Vec<ContainerSummary>, Box<dyn std::error::Error + 'static>> {
        let list_options = ListContainersOptions {
            filters: (&labels).into(),
            all: true,
            ..Default::default()
        };

        Ok(self.client().list_containers(Some(list_options)).await?)
    }

    pub async fn remove_container(
        &self,
        container_id: &str,
        force: bool,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        Ok(self
            .client()
            .remove_container(
                &container_id,
                Some(RemoveContainerOptions {
                    force,
                    ..Default::default()
                }),
            )
            .await?)
    }

    pub async fn stop_container(
        &self,
        container_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        self.client()
            .stop_container(&container_id, Some(StopContainerOptions { t: 0 }))
            .await?;
        let mut count = 10;
        while count > 0 {
            log::debug!("Waiting for container {} to be gone...", container_id);
            let r = self.client().inspect_container(&container_id, None).await;
            if let Err(Error::DockerResponseServerError {
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

    pub async fn create_container(
        &self,
        spec: RunSpec<'a>,
    ) -> Result<ContainerResult, Box<dyn std::error::Error + 'static>> {
        log::debug!(
            "[{}]: Creating container - name: {}, user: {}, image: {}, auto-remove: {}",
            &spec.reason,
            spec.container_name,
            spec.user.unwrap_or_default(),
            spec.image,
            spec.auto_remove,
        );

        let container_id = match self
            .client()
            .inspect_container(&spec.container_name, None)
            .await
        {
            Ok(ContainerInspectResponse { id: Some(id), .. }) if !spec.force_recreate => {
                ContainerResult::AlreadyExists { id }
            }
            s => {
                let remove_options = RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                };

                if let Ok(ContainerInspectResponse { id: Some(id), .. }) = s {
                    self.client()
                        .remove_container(&id, Some(remove_options))
                        .await?;
                }

                let options = CreateContainerOptions {
                    name: spec.container_name,
                    platform: None,
                };

                let host_config = HostConfig {
                    auto_remove: Some(spec.auto_remove),
                    mounts: spec.mounts,
                    privileged: Some(spec.privileged),
                    ..Default::default()
                };

                let mut env_kv = vec![
                    KeyValue::new("ROOZ_META_IMAGE", &spec.image),
                    KeyValue::new("ROOZ_META_WORKSPACE", &spec.workspace_key),
                    KeyValue::new("ROOZ_META_CONTAINER_NAME", &spec.container_name),
                ];

                if let Some(env) = spec.env {
                    env_kv.extend(KeyValue::to_vec(env));
                }

                let env = KeyValue::to_vec_str(&env_kv);

                let config = Config {
                    image: Some(spec.image),
                    entrypoint: spec.entrypoint,
                    working_dir: spec.work_dir,
                    user: spec.user,
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(true),
                    open_stdin: Some(true),
                    host_config: Some(host_config),
                    labels: Some(spec.labels),
                    env: Some(env),
                    ..Default::default()
                };

                let response = self
                    .client()
                    .create_container(Some(options.clone()), config.clone())
                    .await?;

                if let Some(network) = &spec.network {
                    let connect_network_options = ConnectNetworkOptions {
                        container: &response.id,
                        endpoint_config: EndpointSettings {
                            aliases: spec.network_aliases,
                            ..Default::default()
                        },
                    };
                    self.client()
                        .connect_network(network, connect_network_options)
                        .await?;
                }
                log::debug!(
                    "Created container: {} ({})",
                    spec.container_name,
                    response.id
                );

                ContainerResult::Created { id: response.id }
            }
        };
        Ok(container_id.clone())
    }

    pub async fn start_container(
        &self,
        container_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        Ok(self
            .client()
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await?)
    }

    pub async fn container_logs_to_stdout(
        &self,
        container_name: &str,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let log_options = LogsOptions::<String> {
            stdout: true,
            follow: true,
            ..Default::default()
        };

        let mut stream = self.client().logs(&container_name, Some(log_options));

        while let Some(l) = stream.next().await {
            match l {
                Ok(Console { message: m }) => stdout().write_all(&m)?,
                Ok(msg) => panic!("{}", msg),
                Err(e) => panic!("{}", e),
            };
        }
        Ok(())
    }
}
