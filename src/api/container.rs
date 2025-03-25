use std::{collections::HashMap, time::Duration};

use base64::{engine::general_purpose, Engine as _};
use bollard::{
    container::{
        Config, CreateContainerOptions, InspectContainerOptions, KillContainerOptions,
        ListContainersOptions,
        LogOutput::{self},
        LogsOptions, RemoveContainerOptions, StartContainerOptions, StopContainerOptions,
        WaitContainerOptions,
    },
    errors::Error,
    models::{ContainerState, HostConfig},
    network::ConnectNetworkOptions,
    secret::{ContainerStateStatusEnum, Mount},
    service::{ContainerInspectResponse, ContainerSummary, EndpointSettings, PortBinding},
};
use colored::Colorize;
use futures::StreamExt;
use tokio::time::sleep;

use crate::{
    api::ContainerApi,
    constants,
    model::types::{AnyError, ContainerResult, OneShotResult, RunSpec},
    util::{
        backend::ContainerBackend,
        id,
        labels::{KeyValue, Labels},
    },
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

impl<'a> ContainerApi<'a> {
    pub async fn get_all(&self, labels: &Labels) -> Result<Vec<ContainerSummary>, AnyError> {
        let list_options = ListContainersOptions {
            filters: labels.into(),
            all: true,
            ..Default::default()
        };

        Ok(self.client.list_containers(Some(list_options)).await?)
    }

    pub async fn get_running(&self, labels: &Labels) -> Result<Vec<ContainerSummary>, AnyError> {
        let list_options = ListContainersOptions {
            filters: labels.into(),
            all: false,
            ..Default::default()
        };

        Ok(self.client.list_containers(Some(list_options)).await?)
    }

    pub async fn get_single(&self, labels: &Labels) -> Result<Option<ContainerSummary>, AnyError> {
        match self.get_all(&labels).await?.as_slice() {
            [] => Ok(None),
            [container] => Ok(Some(container.clone())),
            _ => panic!("Too many containers found"),
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
                        self.kill(container_id).await?;
                    }
                }
                Err(Error::JsonDataError { message, .. }) => {
                    if message.starts_with("unknown variant `stopped`") {
                        // hack: https://github.com/containers/podman/issues/17728
                        // nothing to kill as the container is already stopped
                        ()
                    } else {
                        panic!("{}", message)
                    }
                }
                Err(e) => panic!("{}", e),
            }
        }

        match self
            .client
            .remove_container(
                &container_id,
                Some(RemoveContainerOptions {
                    force,
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(_) => {
                log::debug!("Removed container: {}{}", &container_id, &force_display);
                Ok(())
            }
            Err(Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                log::debug!(
                    "No such container. Skipping: {}{}",
                    &container_id,
                    &force_display
                );
                Ok(())
            }
            Err(Error::DockerResponseServerError {
                status_code,
                message,
            }) => Err(format!(
                "{} (Error code: {})",
                message.replace("\"", ""),
                status_code
            )
            .into()),
            Err(e) => panic!("{}", e),
        }
    }

    pub async fn kill(&self, container_id: &str) -> Result<(), AnyError> {
        match self
            .client
            .kill_container(&container_id, None::<KillContainerOptions<String>>)
            .await
        {
            Ok(_) => {
                sleep(Duration::from_millis(10)).await;
                Ok(())
            }
            Err(e) => Err(Box::new(e)),
        }
    }

    pub async fn stop(&self, container_id: &str) -> Result<(), AnyError> {
        self.client
            .stop_container(&container_id, Some(StopContainerOptions { t: 0 }))
            .await?;
        let mut count = 10;
        while count > 0 {
            log::debug!("Waiting for container {} to be gone...", container_id);
            let r = self.client.inspect_container(&container_id, None).await;
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

    pub async fn stop_all(&self) -> Result<(), AnyError> {
        let labels = Labels::default();
        for c in self.get_running(&labels).await? {
            print!("Stopping container: {} ... ", c.names.unwrap().join(", "));
            self.stop(&c.id.unwrap()).await?;
            println!("{}", format!("OK").green())
        }
        Ok(())
    }

    pub async fn create(&self, spec: RunSpec<'a>) -> Result<ContainerResult, AnyError> {
        log::debug!(
            "[{}]: Creating container - name: {}, uid: {}, user: {}, image: {}, auto-remove: {}",
            &spec.reason,
            spec.container_name,
            spec.uid,
            spec.user,
            spec.image,
            spec.auto_remove,
        );

        let container_id = match self
            .client
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
                    self.client
                        .remove_container(&id, Some(remove_options))
                        .await?;
                }

                let options = CreateContainerOptions {
                    name: spec.container_name,
                    platform: None,
                };

                let oom_score_adj = match self.backend {
                    ContainerBackend::Podman => Some(100),
                    _ => None,
                };

                let localhost = "127.0.0.1";
                let port_bindings = spec.ports.map(|ports| {
                    let mut bindings = HashMap::<String, Option<Vec<PortBinding>>>::new();

                    for (source, target) in &ports {
                        bindings.insert(
                            source.to_string(),
                            Some(vec![PortBinding {
                                host_port: target.as_deref().map(|x| x.to_string()),
                                host_ip: Some(localhost.to_string()),
                            }]),
                        );
                    }

                    bindings
                });

                let host_config = HostConfig {
                    auto_remove: Some(spec.auto_remove),
                    mounts: spec.mounts,
                    restart_policy: None,
                    oom_score_adj,
                    privileged: Some(spec.privileged),
                    port_bindings,
                    init: Some(true),
                    ..Default::default()
                };

                let mut env_kv = vec![
                    KeyValue::new("ROOZ_META_IMAGE", &spec.image),
                    KeyValue::new("ROOZ_META_UID", &spec.uid.to_string()),
                    KeyValue::new("ROOZ_META_USER", &spec.user),
                    KeyValue::new("ROOZ_META_HOME", &spec.home_dir),
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
                    cmd: spec.command,
                    working_dir: spec.work_dir,
                    user: Some(spec.user),
                    attach_stdin: Some(spec.interactive),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(spec.interactive),
                    open_stdin: Some(spec.interactive),
                    host_config: Some(host_config),
                    labels: Some((&spec.labels).into()),
                    env: Some(env),
                    ..Default::default()
                };

                let response = self
                    .client
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
                    self.client
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

    pub async fn start(&self, container_id: &str) -> Result<(), AnyError> {
        match self
            .client
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| Box::new(e))
        {
            Ok(_) => Ok(()),
            Err(e) => Err(Box::new(e)),
        }
    }

    fn message_string(&self, message: &[u8]) -> String {
        String::from_utf8_lossy(message).trim_end().to_string()
    }

    fn log_line(&self, name: &str, message: &[u8]) {
        println!("{} | {}", name, self.message_string(message))
    }

    pub async fn one_shot_output(
        &self,
        name: &str,
        command: String,
        mounts: Option<Vec<Mount>>,
    ) -> Result<OneShotResult, AnyError> {
        let entrypoint = inject(&command, "entrypoint.sh");
        let id = self
            .create(RunSpec {
                reason: name,
                image: constants::DEFAULT_IMAGE,
                container_name: &id::random_suffix("one-shot"),
                entrypoint: Some(entrypoint.iter().map(String::as_str).collect()),
                auto_remove: true,
                mounts,
                uid: constants::ROOT_UID_INT,
                ..Default::default()
            })
            .await
            .map(|r| r.id().to_string())?;

        self.start(&id).await?;

        let mut exit_code_stream = self
            .client
            .wait_container(&id, None::<WaitContainerOptions<String>>);

        let mut logs_stream = self.client.logs(
            &id,
            Some(LogsOptions::<String> {
                follow: true,
                stdout: true,
                stderr: true,
                ..Default::default()
            }),
        );

        let mut data = Vec::<String>::new();
        while let Some(log_result) = logs_stream.next().await {
            match log_result {
                Ok(LogOutput::Console { message }) => data.push(self.message_string(&message)),
                Ok(LogOutput::StdErr { message }) => data.push(self.message_string(&message)),
                Ok(LogOutput::StdOut { message }) => data.push(self.message_string(&message)),
                Ok(LogOutput::StdIn { .. }) => (),
                Err(e) => {
                    eprintln!("Error getting logs: {:?}", e);
                    break;
                }
            }
        }

        let exit_code = match exit_code_stream.next().await {
            Some(Ok(response)) => response.status_code,
            Some(Err(e)) => return Err(e.into()),
            None => unreachable!("There should always be an exit code"),
        };
        Ok(OneShotResult {
            exit_code,
            data: data.join("\n"),
        })
    }

    pub async fn one_shot(
        &self,
        name: &str,
        command: String,
        mounts: Option<Vec<Mount>>,
    ) -> Result<i64, AnyError> {
        let entrypoint = inject(&command, "entrypoint.sh");
        let id = self
            .create(RunSpec {
                reason: name,
                image: constants::DEFAULT_IMAGE,
                container_name: &id::random_suffix("one-shot"),
                entrypoint: Some(entrypoint.iter().map(String::as_str).collect()),
                auto_remove: true,
                mounts,
                uid: constants::ROOT_UID_INT,
                ..Default::default()
            })
            .await
            .map(|r| r.id().to_string())?;

            self.start(&id).await?;

       

        let mut exit_code_stream = self
            .client
            .wait_container(&id, None::<WaitContainerOptions<String>>);

       
            let mut logs_stream = self.client.logs(
                &id,
                Some(LogsOptions::<String> {
                    follow: true,
                    stdout: true,
                    stderr: true,
                    ..Default::default()
                }),
            );
        while let Some(log_result) = logs_stream.next().await {
            match log_result {
                Ok(LogOutput::Console { message }) => self.log_line(name, &message),
                Ok(LogOutput::StdErr { message }) => self.log_line(name, &message),
                Ok(LogOutput::StdOut { message }) => self.log_line(name, &message),
                Ok(LogOutput::StdIn { .. }) => (),
                Err(e) => {
                    eprintln!("Error getting logs: {:?}", e);
                    break;
                }
            }
        }

        let exit_code = match exit_code_stream.next().await {
            Some(Ok(response)) => response.status_code,
            Some(Err(e)) => return Err(e.into()),
            None => unreachable!("There should always be an exit code"),
        };
        Ok(exit_code)
    }
}
