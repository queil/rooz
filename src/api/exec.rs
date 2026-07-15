use crate::{api::ExecApi, constants, model::types::AnyError};
use bollard::{
    container::LogOutput,
    errors::Error,
    exec::{CreateExecOptions, ResizeExecOptions, StartExecResults},
};
use bollard_stubs::models::ExecInspectResponse;
use futures::{Stream, StreamExt};

use crate::api::container::inject;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::{io::Read, time::Duration};
use tokio::{
    io::{AsyncWriteExt, unix::AsyncFd},
    select, spawn,
    sync::broadcast,
    time::sleep,
};

async fn collect(stream: impl Stream<Item = Result<LogOutput, Error>>) -> Result<String, AnyError> {
    let mut stream = std::pin::pin!(stream);
    let mut out: Vec<u8> = Vec::new();

    while let Some(item) = stream.next().await {
        match item {
            // IMPORTANT: stdout is returned byte-exact (no trimming, no tty
            // CRLF translation) so generated file content round-trips
            Ok(LogOutput::StdOut { message }) | Ok(LogOutput::Console { message }) => {
                out.extend_from_slice(&message)
            }
            Ok(LogOutput::StdErr { message }) => {
                log::debug!("stderr | {}", String::from_utf8_lossy(&message).trim_end())
            }
            Ok(_) => {}
            Err(err) => return Err(err.into()),
        }
    }

    Ok(String::from_utf8(out)?)
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
        S: Stream<Item = Result<LogOutput, Error>> + Unpin,
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

    async fn start_tty(&self, exec_id: &str) -> Result<(), AnyError> {
        let (width, height) = crossterm::terminal::size()?;
        if let StartExecResults::Attached { output, mut input } =
            self.client.start_exec(exec_id, None).await?
        {
            let exec_state = self.client.inspect_exec(&exec_id).await?;

            match exec_state.clone() {
                ExecInspectResponse {
                    running: Some(true),
                    ..
                } => {
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

                ExecInspectResponse {
                    exit_code: Some(exit_code),
                    ..
                } => {
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
        interactive: bool,
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
                        attach_stdin: Some(interactive),
                        tty: Some(interactive),
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
        working_dir: Option<&str>,
        user: Option<&str>,
        cmd: Option<Vec<&str>>,
    ) -> Result<(), AnyError> {
        let exec_id = self
            .create_exec(reason, container_id, working_dir, user, cmd, true)
            .await?;

        self.start_tty(&exec_id).await
    }

    pub async fn output(
        &self,
        reason: &str,
        container_id: &str,
        user: Option<&str>,
        cmd: Option<Vec<&str>>,
    ) -> Result<String, AnyError> {
        let exec_id = self
            .create_exec(reason, container_id, None, user, cmd, false)
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
            .create_exec(reason, container_id, None, user, cmd, false)
            .await?;
        if let StartExecResults::Attached { output, .. } =
            self.client.start_exec(&exec_id, None).await?
        {
            log(output).await?;
        } else {
            panic!("Could not start exec");
        }

        if let ExecInspectResponse {
            exit_code: Some(exit_code),
            ..
        } = self.client.inspect_exec(&exec_id).await?
            && exit_code != 0
        {
            return Err(format!("{}: exec failed with exit code {}", reason, exit_code).into());
        }
        Ok(())
    }

    pub async fn install(
        &self,
        container_name: &str,
        container_id: &str,
        steps: Vec<(String, String)>,
    ) -> Result<(), AnyError> {
        let step_names = steps
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        println!("[install] {} steps: {}", container_name, step_names);
        for (idx, (name, cmd)) in steps.iter().enumerate() {
            let script = if cmd.starts_with("#!") {
                cmd.splitn(2, '\n').nth(1).unwrap_or(cmd)
            } else {
                cmd
            };
            let cmd = format!(
                r#"#!/bin/sh
set -e
echo '[install] {}: {}'
{}"#,
                container_name, name, script
            );
            let install_cmd = inject(cmd.as_str(), &format!("install-{}.sh", idx));
            let v = install_cmd.iter().map(|x| x.as_str()).collect::<Vec<_>>();
            self.run("install", container_id, Some(constants::ROOT_UID), Some(v))
                .await
                .map_err(|e| -> AnyError {
                    format!("install step '{}' failed: {}", name, e).into()
                })?;
        }
        Ok(())
    }

    pub async fn chown(&self, container_id: &str, uid: &i32, dir: &str) -> Result<(), AnyError> {
        // TODO: this is true if the volume was first mounted as the right user
        // otherwise chown is still needed. I could probably check if a setup container had run first
        // because this is the main reason the volumes would be owned by root

        // if let ContainerEngine::Podman = self.backend.engine {
        //    log::debug!("Podman won't need chown. Skipping");
        //    return Ok(());
        //};

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
        let ensure_user_cmd = inject(
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
