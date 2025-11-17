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
    errors::Error::{self, DockerResponseServerError},
    models::{
        ContainerCreateBody, ContainerCreateResponse, ContainerInspectResponse, ContainerState,
        ContainerStateStatusEnum, ContainerSummary, EndpointSettings, HostConfig, Mount,
        NetworkConnectRequest, PortBinding,
    },
    query_parameters::{
        CreateContainerOptions, InspectContainerOptions, KillContainerOptions,
        ListContainersOptions, LogsOptions, RemoveContainerOptions, StartContainerOptions,
        StopContainerOptions,
    },
};

use futures::{StreamExt, future};
use std::{collections::HashMap, time::Duration};
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
                Err(DockerResponseServerError {
                    status_code: 404,
                    message,
                }) => {
                    log::debug!("Container no longer exists: {}", message);
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
            .kill_container(&container_id, None::<KillContainerOptions>)
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
            .stop_container(
                &container_id,
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
                .inspect_container(&container_id, None::<InspectContainerOptions>)
                .await;
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

    async fn create_core(&self, spec: RunSpec<'a>) -> Result<ContainerCreateResponse, AnyError> {
        let image_info = self.image.ensure(&spec.image, spec.force_pull).await?;

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

        let (attach_stdin, tty, open_stdin, auto_remove) = match spec.run_mode {
            RunMode::Workspace => (Some(true), Some(true), None, None),
            RunMode::Tmp => (Some(true), Some(true), None, Some(true)),
            RunMode::Git => (None, None, Some(true), Some(true)),
            RunMode::OneShot => (None, None, None, Some(true)),
            RunMode::Sidecar => (None, None, None, None),
        };

        let host_config = HostConfig {
            auto_remove,
            mounts: spec.mounts,
            restart_policy: None,
            oom_score_adj,
            privileged: Some(spec.privileged),
            init: Some(spec.init),
            port_bindings,
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

        let config = ContainerCreateBody {
            image: Some(spec.image.to_string()),
            entrypoint: spec
                .entrypoint
                .map(|vec| vec.iter().map(|&s| s.to_string()).collect()),
            cmd: spec
                .command
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
            ..Default::default()
        };

        let response = match self
            .client
            .create_container(Some(options.clone()), config.clone())
            .await
        {
            Ok(r) => r,
            Err(err) => panic!("ERROR: {:?}", err),
        };

        if let Some(network) = &spec.network {
            let connect_network_options = NetworkConnectRequest {
                container: Some(response.id.to_string()),
                endpoint_config: Some(EndpointSettings {
                    aliases: spec.network_aliases,
                    ..Default::default()
                }),
            };
            self.client
                .connect_network(network, connect_network_options)
                .await?;
        }
        log::debug!(
            "Created container: {} ({})",
            spec.container_name,
            response.id.to_string()
        );
        Ok(response)
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
            .inspect_container(&spec.container_name, None::<InspectContainerOptions>)
            .await
        {
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
            Err(err) => panic!("ERROR: {:?}", err),
        };
        Ok(container_id.clone())
    }

    pub async fn start(&self, container_id: &str) -> Result<(), bollard::errors::Error> {
        self.client
            .start_container(&container_id, None::<StartContainerOptions>)
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
TIMEOUT=${EXEC_TIMEOUT:-300}
mkfifo /tmp/exec_start /tmp/exec_end

echo "Waiting for exec session (timeout: ${TIMEOUT}s)..."
timeout $TIMEOUT sh -c 'read _ < /tmp/exec_start' || exit 1

echo "Exec session started"
read _ < /tmp/exec_end
echo "Exec session ended"
exit 0"#;

        let epv = inject(&wait_for_exec, "entrypoint.sh");
        let entrypoint = epv.iter().map(String::as_str).collect();
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
                            Err(err) => panic!("{}", err),
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
trap 'echo end > /tmp/exec_end' EXIT
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
    ) -> Result<OneShotResult, AnyError> {
        let id = self.make_one_shot(name, mounts, uid, None).await?;
        let cmd = Self::format_cmd(command);
        let cmd = cmd.iter().map(|x| x.as_str()).collect::<Vec<_>>();
        let data = self.exec.output(name, &id.clone(), uid, Some(cmd)).await?;

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
        self.exec.run(name, &id, uid, Some(cmd)).await
    }
}
