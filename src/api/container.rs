use crate::{
    api::ContainerApi,
    constants,
    model::types::{AnyError, ContainerResult, OneShotResult, RunMode, RunSpec},
    util::{
        backend::ContainerBackend,
        id,
        labels::{KeyValue, Labels},
    },
};
use base64::{engine::general_purpose, Engine as _};
use bollard::container::LogOutput::Console;
use bollard::{
    container::{
        Config, CreateContainerOptions, InspectContainerOptions, KillContainerOptions,
        ListContainersOptions, LogsOptions, RemoveContainerOptions, StartContainerOptions,
        StopContainerOptions, WaitContainerOptions,
    },
    errors::Error,
    models::{ContainerState, HostConfig},
    network::ConnectNetworkOptions,
    secret::{ContainerStateStatusEnum, Mount},
    service::{ContainerInspectResponse, ContainerSummary, EndpointSettings, PortBinding},
};
use futures::{future, StreamExt};
use std::{
    collections::HashMap,
    io::{stdout, Write},
    time::Duration,
};
use tokio::time::{sleep, timeout};

pub fn inject2(script: &str, name: &str, post_sleep: bool) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "echo '{}' | base64 -d > /tmp/{} && chmod +x /tmp/{} && /tmp/{}{}",
            general_purpose::STANDARD.encode(script.trim()),
            name,
            name,
            name,
            if post_sleep { " && sleep 0.5" } else { "" }
        ),
    ]
}

pub fn inject(script: &str, name: &str) -> Vec<String> {
    inject2(&script, &name, false)
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
                        self.kill(container_id, true).await?;
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

    pub async fn kill(&self, container_id: &str, wait_for_remove: bool) -> Result<(), AnyError> {
        match self
            .client
            .kill_container(&container_id, None::<KillContainerOptions<String>>)
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
                                        ()
                                    } else {
                                        panic!("{}", message)
                                    }
                                }
                                //Podman backend
                                Err(Error::DockerResponseServerError {
                                    status_code: 500,
                                    message,
                                }) if message.ends_with("no such container") => return Ok(()),
                                //Docker backend
                                Err(Error::DockerResponseServerError {
                                    status_code: 404, ..
                                }) => return Ok(()),
                                Err(e) => panic!("{}", e),
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

    pub async fn create(&self, spec: RunSpec<'a>) -> Result<ContainerResult, AnyError> {
        log::debug!(
            "[{}: {:?}]: CREATE CONTAINER - name: {}, uid: {}, user: {}, image: {}, entrypoint: {}",
            &spec.reason,
            spec.run_mode,
            spec.container_name,
            spec.uid,
            spec.user,
            spec.image,
            spec.entrypoint.clone().unwrap_or(vec![]).join(" ")
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

                let (attach_stdin, tty, open_stdin, auto_remove) = match spec.run_mode {
                    RunMode::Workspace => (Some(true), Some(true), None, None),
                    RunMode::Tmp => (Some(true), Some(true), None, Some(true)),
                    RunMode::Git => (None, None, Some(true), Some(true)),
                    RunMode::OneShot => (None, None, None, Some(true)),
                    RunMode::Sidecar => (None, None, None, None),
                    RunMode::Init => (None, None, None, None),
                };

                let host_config = HostConfig {
                    auto_remove,
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
                    // THIS MUST BE spec.uid, NOT spec.user - otherwise file ownership will break
                    user: Some(spec.uid),
                    attach_stdin,
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty,
                    open_stdin,
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

    async fn make_one_shot(
        &self,
        name: &str,
        command: String,
        mounts: Option<Vec<Mount>>,
        uid: Option<&str>,
        image: Option<&str>,
    ) -> Result<String, AnyError> {
        let entrypoint_v = inject2(&command, "entrypoint.sh", true);
        let entrypoint = entrypoint_v.iter().map(String::as_str).collect();
        let id = self
            .create(RunSpec {
                reason: name,
                image: image.unwrap_or(constants::DEFAULT_IMAGE),
                container_name: &id::random_suffix("one-shot"),
                entrypoint: Some(entrypoint),
                mounts,
                uid: uid.unwrap_or(constants::ROOT_UID),
                ..Default::default()
            })
            .await
            .map(|r| r.id().to_string())?;

        self.start(&id).await?;
        Ok(id)
    }

    pub async fn one_shot_output(
        &self,
        name: &str,
        command: String,
        mounts: Option<Vec<Mount>>,
        uid: Option<&str>,
    ) -> Result<OneShotResult, AnyError> {
        let id = self.make_one_shot(name, command, mounts, uid, None).await?;
        let docker_logs = self.client.clone();
        let s_id = id.clone();

        let log_task = tokio::spawn(async move {
            let logs_stream = docker_logs.logs(
                &s_id,
                Some(LogsOptions::<String> {
                    follow: true,
                    stdout: true,
                    stderr: true,
                    ..Default::default()
                }),
            );

            let data = logs_stream
                .map(|x| match x {
                    Ok(r) => String::from_utf8_lossy(r.into_bytes().as_ref())
                        .to_string()
                        .trim_end()
                        .to_string(),
                    Err(err) => panic!("{}", err),
                })
                .collect::<Vec<_>>()
                .await
                .join("\n");
            data
        });

        let mut exit_code_stream = self
            .client
            .wait_container(&id, None::<WaitContainerOptions<String>>);

        let _ = match exit_code_stream.next().await {
            Some(Ok(response)) => response.status_code,
            Some(Err(e)) => return Err(e.into()),
            None => unreachable!("Container exited without status code"),
        };

        let data = tokio::time::timeout(std::time::Duration::from_secs(2), log_task).await??;
        Ok(OneShotResult { data })
    }

    pub async fn one_shot(
        &self,
        name: &str,
        command: String,
        mounts: Option<Vec<Mount>>,
        uid: Option<&str>,
        image: Option<&str>,
    ) -> Result<i64, AnyError> {
        let id = self
            .make_one_shot(name, command, mounts, uid, image)
            .await?;
        let docker_logs = self.client.clone();
        let s_id = id.clone();
        let s_name = name.to_string();

        let log_task = tokio::spawn(async move {
            let logs_stream = docker_logs.logs(
                &s_id,
                Some(LogsOptions::<String> {
                    follow: true,
                    stdout: true,
                    stderr: true,
                    ..Default::default()
                }),
            );

            logs_stream
                .for_each(|x| {
                    match x {
                        Ok(r) => println!(
                            "{} | {}",
                            s_name,
                            String::from_utf8_lossy(r.into_bytes().as_ref())
                                .to_string()
                                .trim_end()
                        ),
                        Err(err) => panic!("{}", err),
                    };
                    future::ready(())
                })
                .await;
        });

        let mut exit_code_stream = self
            .client
            .wait_container(&id, None::<WaitContainerOptions<String>>);

        let exit_code = match exit_code_stream.next().await {
            Some(Ok(response)) => response.status_code,
            Some(Err(e)) => return Err(e.into()),
            None => unreachable!("Container exited without status code"),
        };

        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), log_task).await;
        Ok(exit_code)
    }

    pub async fn logs_to_stdout(&self, container_name: &str) -> Result<(), AnyError> {
        let log_options = LogsOptions::<String> {
            stdout: true,
            follow: true,
            ..Default::default()
        };

        let mut stream = self.client.logs(&container_name, Some(log_options));

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
