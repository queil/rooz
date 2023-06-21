use std::{
    io::{stdin, stdout, Write},
    time::Duration,
};

use base64::{engine::general_purpose, Engine as _};
use bollard::{
    container::{
        Config, CreateContainerOptions, ListContainersOptions, LogOutput,
        LogOutput::Console, LogsOptions, RemoveContainerOptions, StartContainerOptions,
        StopContainerOptions,
    },
    errors::Error,
    exec::{CreateExecOptions, ResizeExecOptions, StartExecResults},
    models::HostConfig,
    network::ConnectNetworkOptions,
    service::{ContainerInspectResponse, ContainerSummary, EndpointSettings},
    Docker,
};
use futures::{channel::oneshot, Stream, StreamExt};
use nonblock::NonBlockingReader;
use termion::{raw::IntoRawMode, terminal_size};
use tokio::{io::AsyncWriteExt, spawn, time::sleep};

use crate::{
    labels::{KeyValue, Labels},
    types::{ContainerResult, RunSpec}, constants, backend::ContainerBackend,
};

async fn start_tty(
    docker: &Docker,
    exec_id: &str,
    interactive: bool,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let tty_size = terminal_size()?;
    if let StartExecResults::Attached {
        mut output,
        mut input,
    } = docker.start_exec(exec_id, None).await?
    {
        let (r, mut s) = oneshot::channel::<bool>();
        let handle = spawn(async move {
            if interactive {
                let mut stdin = NonBlockingReader::from_fd(stdin()).unwrap();
                loop {
                    let mut bytes = Vec::new();
                    match stdin.read_available(&mut bytes).ok() {
                        Some(c) if c > 0 => {
                            input.write_all(&bytes).await.ok();
                        }
                        _ => {
                            if let Some(true) = s.try_recv().unwrap() {
                                break;
                            }
                            sleep(Duration::from_millis(10)).await;
                        }
                    }
                }
            }
        });

        if interactive {
            match docker
                .resize_exec(
                    exec_id,
                    ResizeExecOptions {
                        height: tty_size.1,
                        width: tty_size.0,
                    },
                )
                .await
            {
                Ok(_) => (),
                Err(err) => println!("Resize exec: {:?}", err),
            };
            println!("{}", termion::clear::All);
        };

        // set stdout in raw mode so we can do tty stuff
        let stdout = stdout();
        let mut stdout = stdout.lock().into_raw_mode()?;
        // pipe docker exec output into stdout
        while let Some(Ok(output)) = output.next().await {
            stdout.write_all(output.into_bytes().as_ref())?;
            stdout.flush()?;
        }

        if interactive {
            r.send(true).ok();
            handle.await?;
        }
    }
    Ok(())
}

async fn exec(
    reason: &str,
    docker: &Docker,
    container_id: &str,
    working_dir: Option<&str>,
    user: Option<&str>,
    cmd: Option<Vec<&str>>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    #[cfg(not(windows))]
    {
        log::debug!(
            "[{}] exec: {:?} in working dir: {:?}",
            reason,
            cmd,
            working_dir
        );

        Ok(docker
            .create_exec(
                &container_id,
                CreateExecOptions {
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    attach_stdin: Some(true),
                    tty: Some(true),
                    cmd,
                    working_dir,
                    user,
                    ..Default::default()
                },
            )
            .await?
            .id)
    }
}

pub async fn exec_tty(
    reason: &str,
    docker: &Docker,
    container_id: &str,
    interactive: bool,
    working_dir: Option<&str>,
    user: Option<&str>,
    cmd: Option<Vec<&str>>,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let exec_id = exec(reason, docker, container_id, working_dir, user, cmd).await?;
    start_tty(docker, &exec_id, interactive).await
}

async fn collect(
    stream: impl Stream<Item = Result<LogOutput, Error>>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let out = stream
        .map(|x| match x {
            Ok(r) => std::str::from_utf8(r.into_bytes().as_ref())
                .unwrap()
                .to_string(),
            Err(err) => panic!("{}", err),
        })
        .collect::<Vec<_>>()
        .await
        .join("");

    let trimmed = out.trim();
    Ok(trimmed.to_string())
}

pub async fn exec_output(
    reason: &str,
    docker: &Docker,
    container_id: &str,
    user: Option<&str>,
    cmd: Option<Vec<&str>>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let exec_id = exec(reason, docker, container_id, None, user, cmd).await?;
    if let StartExecResults::Attached { output, .. } = docker.start_exec(&exec_id, None).await? {
        collect(output).await
    } else {
        panic!("Could not start exec");
    }
}

pub async fn get_all(
    docker: &Docker,
    labels: Labels,
) -> Result<Vec<ContainerSummary>, Box<dyn std::error::Error + 'static>> {
    let list_options = ListContainersOptions {
        filters: (&labels).into(),
        all: true,
        ..Default::default()
    };

    Ok(docker.list_containers(Some(list_options)).await?)
}

pub async fn remove(
    docker: &Docker,
    container_id: &str,
    force: bool,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    Ok(docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force,
                ..Default::default()
            }),
        )
        .await?)
}

pub async fn stop(
    docker: &Docker,
    container_id: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    docker
        .stop_container(&container_id, Some(StopContainerOptions { t: 0 }))
        .await?;
    let mut count = 10;
    while count > 0 {
        log::debug!("Waiting for container {} to be gone...", container_id);
        let r = docker.inspect_container(&container_id, None).await;
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

pub async fn create<'a>(
    docker: &Docker,
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

    let container_id = match docker.inspect_container(&spec.container_name, None).await {
        Ok(ContainerInspectResponse { id: Some(id), .. }) if !spec.force_recreate => {
            ContainerResult::AlreadyExists { id }
        }
        s => {
            let remove_options = RemoveContainerOptions {
                force: true,
                ..Default::default()
            };

            if let Ok(ContainerInspectResponse { id: Some(id), .. }) = s {
                docker.remove_container(&id, Some(remove_options)).await?;
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

            let response = docker
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
                docker
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

pub async fn start(
    docker: &Docker,
    container_id: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    Ok(docker
        .start_container(&container_id, None::<StartContainerOptions<String>>)
        .await?)
}

pub async fn container_logs_to_stdout(
    docker: &Docker,
    container_name: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let log_options = LogsOptions::<String> {
        stdout: true,
        follow: true,
        ..Default::default()
    };

    let mut stream = docker.logs(&container_name, Some(log_options));

    while let Some(l) = stream.next().await {
        match l {
            Ok(Console { message: m }) => stdout().write_all(&m)?,
            Ok(msg) => panic!("{}", msg),
            Err(e) => panic!("{}", e),
        };
    }
    Ok(())
}

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

pub async fn chown(docker: &Docker, container_id: &str, uid: &str, dir: &str)
    -> Result<(), Box<dyn std::error::Error + 'static>>
{
    if let ContainerBackend::Podman = ContainerBackend::resolve(docker).await? {
        log::debug!("Podman won't need chown. Skipping");
        return Ok(())
    };

    let uid_format = format!("{}:{}", &uid, &uid);
        let chown_response = exec_output(
            "chown",
            docker,
            container_id,
            Some(constants::ROOT),
            Some(vec!["chown", "-R", &uid_format, &dir]),
        )
        .await?;

        log::debug!("{}", chown_response);
        Ok(())
}
