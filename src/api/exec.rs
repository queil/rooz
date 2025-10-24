use crate::{
    api::container, api::ExecApi, constants, model::types::AnyError, util::backend::ContainerEngine,
};
use bollard::{
    container::LogOutput,
    errors::Error,
    exec::{CreateExecOptions, ResizeExecOptions, StartExecResults},
    secret::ExecInspectResponse,
};
use futures::{Stream, StreamExt};

use std::{io::Read, time::Duration};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use tokio::{
    io::{unix::AsyncFd, AsyncWriteExt},
    select, spawn,
    sync::broadcast,
    time::sleep,
};

async fn collect(stream: impl Stream<Item = Result<LogOutput, Error>>) -> Result<String, AnyError> {
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

async fn log(stream: impl Stream<Item = Result<LogOutput, Error>>) -> Result<(), AnyError> {
    use futures::stream::StreamExt;
    use std::io::Write;

    let mut stream = std::pin::pin!(stream);
    let mut stdout = std::io::stdout();

    while let Some(item) = stream.next().await {
        match item {
            Ok(output) => {
                let bytes = output.into_bytes();
                let text = std::str::from_utf8(bytes.as_ref())?;
                stdout.write_all(text.as_bytes())?;
                stdout.flush()?;
            }
            Err(err) => return Err(err.into()),
        }
    }

    Ok(())
}

impl<'a> ExecApi<'a> {
    async fn handle_output<S>(&self, mut output: S)
    where
        S: Stream<Item = Result<LogOutput, bollard::errors::Error>> + Unpin,
    {
        let mut stdout = tokio::io::stdout();
        while let Some(Ok(out)) = output.next().await {
            let bytes = out.into_bytes();

            while let Err(_) = stdout.write_all(&bytes).await {
                sleep(Duration::from_millis(10)).await;
            }

            while let Err(_) = stdout.flush().await {
                sleep(Duration::from_millis(10)).await;
            }
        }
    }

    async fn start_tty(&self, exec_id: &str, interactive: bool) -> Result<(), AnyError> {
        let (width, height) = crossterm::terminal::size()?;
        if let StartExecResults::Attached { output, mut input } =
            self.client.start_exec(exec_id, None).await?
        {
            let exec_state = self.client.inspect_exec(&exec_id).await?;

            match (exec_state.clone(), interactive) {
                (
                    ExecInspectResponse {
                        running: Some(true),
                        ..
                    },
                    true,
                ) => {
                    enable_raw_mode()?;

                    let stdin_reader = std::io::stdin();
                    let async_stdin = AsyncFd::new(stdin_reader)?;

                    let mut buffer = [0; 1024];
                    let (s, mut r) = broadcast::channel::<bool>(1);
                    let handle = spawn(async move {
                        loop {
                            select! {
                                guard = async_stdin.readable() => {
                                    let mut guard = guard?;
                                    match guard.try_io(|inner| inner.get_ref().read(&mut buffer)) {
                                        Ok(Ok(n)) => {
                                            input.write_all(&buffer[..n]).await?;
                                            guard.clear_ready();
                                        }
                                        _ => {}
                                    }
                                }

                              _ = r.recv() => {break}
                              _ = sleep(Duration::from_millis(10)) => { }

                            }
                        }
                        Result::<(), AnyError>::Ok(())
                    });

                    self.client
                        .resize_exec(exec_id, ResizeExecOptions { height, width })
                        .await
                        .inspect_err(|e| log::debug!("Exec might have already terminated: {}", e))
                        .ok();

                    self.handle_output(output).await;

                    s.send(true).ok();

                    handle.await??;
                    disable_raw_mode()?;
                    // try ping to see if the connection was lost
                    // if this fails the calling code loops retrying to connect to the session
                    self.client.ping().await?;
                }
                (
                    ExecInspectResponse {
                        running: Some(true),
                        ..
                    },
                    false,
                ) => {
                    self.handle_output(output).await;
                }
                (
                    ExecInspectResponse {
                        exit_code: Some(exit_code),
                        ..
                    },
                    _,
                ) => {
                    self.handle_output(output).await;
                    if exit_code != 0 {
                        panic!("Exec terminated with exit code: {}.", exit_code);
                    }
                }
                _ => panic!("Unexpected exec state: {:?}", exec_state.clone()),
            };
        }
        Ok(())
    }

    async fn create_exec(
        &self,
        reason: &str,
        container_id: &str,
        working_dir: Option<&str>,
        user: Option<&str>,
        cmd: Option<Vec<&str>>,
    ) -> Result<String, AnyError> {
        #[cfg(not(windows))]
        {
            log::debug!(
                "[{}] exec: {:?} in working dir: {:?}",
                reason,
                cmd,
                working_dir
            );

            Ok(self
                .client
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

    pub async fn tty(
        &self,
        reason: &str,
        container_id: &str,
        interactive: bool,
        working_dir: Option<&str>,
        user: Option<&str>,
        cmd: Option<Vec<&str>>,
    ) -> Result<(), AnyError> {
        let exec_id = self
            .create_exec(reason, container_id, working_dir, user, cmd)
            .await?;

        self.start_tty(&exec_id, interactive).await
    }

    pub async fn output(
        &self,
        reason: &str,
        container_id: &str,
        user: Option<&str>,
        cmd: Option<Vec<&str>>,
    ) -> Result<String, AnyError> {
        let exec_id = self
            .create_exec(reason, container_id, None, user, cmd)
            .await?;
        if let StartExecResults::Attached { output, .. } =
            self.client.start_exec(&exec_id, None).await?
        {
            collect(output).await
        } else {
            panic!("Could not start exec");
        }
    }

    pub async fn run(
        &self,
        reason: &str,
        container_id: &str,
        user: Option<&str>,
        cmd: Option<Vec<&str>>,
    ) -> Result<(), AnyError> {
        let exec_id = self
            .create_exec(reason, container_id, None, user, cmd)
            .await?;
        if let StartExecResults::Attached { output, .. } =
            self.client.start_exec(&exec_id, None).await?
        {
            log(output).await?;
            Ok(())
        } else {
            panic!("Could not start exec");
        }
    }

    pub async fn chown(&self, container_id: &str, uid: &str, dir: &str) -> Result<(), AnyError> {
        if let ContainerEngine::Podman = self.backend.engine {
            log::debug!("Podman won't need chown. Skipping");
            return Ok(());
        };

        log::debug!("Changing ownership... ({} {})", &uid, &dir);

        let uid_format = format!("{}:{}", &uid, &uid);
        let chown_response = self
            .output(
                "chown",
                container_id,
                Some(constants::ROOT_USER),
                Some(vec![
                    "sh",
                    "-c",
                    &format!(
                        "chown -R {} {}",
                        &uid_format,
                        &dir.replace("~", "${ROOZ_META_HOME}")
                    ),
                ]),
            )
            .await?;

        log::debug!("{}", chown_response);
        Ok(())
    }

    pub async fn ensure_user(&self, container_id: &str) -> Result<(), AnyError> {
        let ensure_user_cmd = container::inject(
            format!(
                    r#"grep -q "^$ROOZ_META_USER:x:$ROOZ_META_UID" /etc/passwd && exit 0
                       sed -i "/:x:${{ROOZ_META_UID}}/d" /etc/passwd && \
                       echo "$ROOZ_META_USER:x:$ROOZ_META_UID:$ROOZ_META_UID:$ROOZ_META_USER:$ROOZ_META_HOME:/bin/sh" >> /etc/passwd"#, 
            )
            .as_ref(),
            "make_user.sh",
        );

        let ensure_user_output = self
            .output(
                "ensure-user",
                container_id,
                Some(constants::ROOT_UID),
                Some(ensure_user_cmd.iter().map(String::as_str).collect()),
            )
            .await?;
        log::debug!("{}", &ensure_user_output);
        Ok(())
    }
}
