use std::{
    collections::HashMap,
    io::{stdout, Read, Write},
    time::Duration,
};

use base64::{engine::general_purpose, Engine as _};
use bollard::{
    container::{
        Config, CreateContainerOptions, LogOutput, LogOutput::Console, LogsOptions,
        RemoveContainerOptions, StartContainerOptions,
    },
    errors::Error,
    exec::{CreateExecOptions, ResizeExecOptions, StartExecResults},
    models::HostConfig,
    service::ContainerInspectResponse,
    Docker,
};
use futures::{Stream, StreamExt};
use termion::{async_stdin, raw::IntoRawMode, terminal_size};
use tokio::{io::AsyncWriteExt, spawn, time::sleep};

use crate::types::{ContainerResult, RunSpec};

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
        // pipe stdin into the docker exec stream input
        let handle = spawn(async move {
            if interactive {
                let mut stdin = async_stdin().bytes();
                loop {
                    if let Some(Ok(byte)) = stdin.next() {
                        input.write(&[byte]).await.ok();
                    } else {
                        sleep(Duration::from_millis(10)).await;
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

        handle.abort();
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

pub async fn force_remove(
    docker: &Docker,
    container_id: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    Ok(docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await?)
}

pub async fn run<'a>(
    docker: &Docker,
    spec: RunSpec<'a>,
) -> Result<ContainerResult, Box<dyn std::error::Error + 'static>> {
    log::debug!(
        "[{}]: running {} as {:?} using image {}",
        &spec.reason,
        spec.container_name,
        spec.user,
        spec.image
    );

    let container_id = match docker.inspect_container(&spec.container_name, None).await {
        Ok(ContainerInspectResponse {
            id: Some(id),
            image: Some(img),
            ..
        }) if img.to_owned() == spec.image_id => ContainerResult::Reused { id },
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
                auto_remove: Some(true),
                mounts: spec.mounts,
                privileged: Some(spec.privileged),
                ..Default::default()
            };

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
                labels: Some(HashMap::from([("dev.rooz", "true")])),
                ..Default::default()
            };

            ContainerResult::Created {
                id: docker
                    .create_container(Some(options.clone()), config.clone())
                    .await?
                    .id,
            }
        }
    };

    docker
        .start_container(&container_id.id(), None::<StartContainerOptions<String>>)
        .await?;

    Ok(container_id.clone())
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
