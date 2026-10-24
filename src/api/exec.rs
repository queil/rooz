use crate::{
    api::ExecApi,
    config::config::ResolvedStep,
    constants,
    model::types::AnyError,
    util::{live, sh},
};
use bollard::{
    container::LogOutput,
    errors::Error,
    exec::{CreateExecOptions, ResizeExecOptions, StartExecResults},
};
use bollard_stubs::models::ExecInspectResponse;
use futures::{Stream, StreamExt};

use crate::api::checkpoint::Checkpoints;
use crate::api::container::{inject, redact_injected};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::{io::Read, time::Duration};
use tokio::{
    io::{AsyncWriteExt, unix::AsyncFd},
    select,
    signal::unix::{SignalKind, signal},
    spawn,
    sync::broadcast,
    time::sleep,
};

/// Where an install saves its progress, and how much of it an earlier attempt already made.
pub struct InstallCheckpoints<'a> {
    pub checkpoints: &'a Checkpoints,
    pub done: usize,
}

/// A command that ran and exited non-zero - as opposed to one that could not run at all.
#[derive(Debug)]
pub struct ExecExit(pub i64);

impl std::fmt::Display for ExecExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Command exited with code {}.", self.0)
    }
}

impl std::error::Error for ExecExit {}

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

// An unterminated run by one step - a progress bar, or a step that never prints a newline -
// must not grow the buffer with everything it printed.
const MAX_PENDING: usize = 64 * 1024;

fn trim_unterminated(pending: &mut Vec<u8>) {
    if pending.len() > MAX_PENDING {
        // what a terminal shows of an unterminated line is the text after its last carriage
        // return; drop what precedes it, then cap what is left
        let from = pending
            .iter()
            .rposition(|b| *b == b'\r')
            .map_or(0, |cr| cr + 1);
        let keep_from = from.max(pending.len() - MAX_PENDING);
        pending.drain(..keep_from);
    }
}

// Puts the terminal back however the session ends: an early `?` must not leave it raw.
struct RawMode;

impl RawMode {
    fn enable() -> std::io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        disable_raw_mode().ok();
    }
}

// With an install (container, step), output goes out line by line through the live view:
// install steps of several containers run at once, and their output interleaves.
async fn log(
    stream: impl Stream<Item = Result<LogOutput, Error>>,
    install: Option<(&str, &str)>,
) -> Result<(), AnyError> {
    use futures::stream::StreamExt;
    use std::io::Write;

    let mut stream = std::pin::pin!(stream);
    let mut pending = Vec::<u8>::new();

    while let Some(item) = stream.next().await {
        let bytes = item?.into_bytes();
        let Some((container, step)) = install else {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(bytes.as_ref())?;
            stdout.flush()?;
            continue;
        };
        pending.extend_from_slice(&bytes);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line = pending.drain(..=end).collect::<Vec<_>>();
            live::install_output(
                container,
                step,
                String::from_utf8_lossy(&line[..line.len() - 1]).as_ref(),
            );
        }
        trim_unterminated(&mut pending);
    }
    if let Some((container, step)) = install
        && !pending.is_empty()
    {
        live::install_output(container, step, String::from_utf8_lossy(&pending).as_ref());
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

            while stdout.write_all(&bytes).await.is_err() {
                sleep(Duration::from_millis(10)).await;
            }

            while stdout.flush().await.is_err() {
                sleep(Duration::from_millis(10)).await;
            }
        }
    }

    async fn start_tty(&self, exec_id: &str) -> Result<(), AnyError> {
        let (width, height) = crossterm::terminal::size()?;
        if let StartExecResults::Attached { output, mut input } =
            self.client.start_exec(exec_id, None).await?
        {
            let exec_state = self.client.inspect_exec(exec_id).await?;

            match exec_state.clone() {
                ExecInspectResponse {
                    running: Some(true),
                    ..
                } => {
                    let raw_mode = RawMode::enable()?;

                    let stdin_reader = std::io::stdin();
                    let async_stdin = AsyncFd::new(stdin_reader)?;

                    let mut buffer = [0; 1024];
                    let (s, mut r) = broadcast::channel::<bool>(1);
                    let handle = spawn(async move {
                        loop {
                            select! {
                                guard = async_stdin.readable() => {
                                    let mut guard = guard?;
                                    if let Ok(Ok(n)) = guard.try_io(|inner| inner.get_ref().read(&mut buffer)) {
                                        input.write_all(&buffer[..n]).await?;
                                        guard.clear_ready();
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

                    // follow terminal resizes for the rest of the session
                    let mut winch = signal(SignalKind::window_change())?;
                    let mut resize_stop = s.subscribe();
                    let resize_client = self.client.clone();
                    let resize_exec_id = exec_id.to_string();
                    let resize_handle = spawn(async move {
                        loop {
                            select! {
                                _ = winch.recv() => {
                                    if let Ok((width, height)) = crossterm::terminal::size() {
                                        resize_client
                                            .resize_exec(&resize_exec_id, ResizeExecOptions { height, width })
                                            .await
                                            .inspect_err(|e| log::debug!("Resize failed: {}", e))
                                            .ok();
                                    }
                                }
                                _ = resize_stop.recv() => break,
                            }
                        }
                    });

                    self.handle_output(output).await;

                    s.send(true).ok();

                    handle.await??;
                    resize_handle.await.ok();
                    drop(raw_mode);
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
                        return Err(ExecExit(exit_code).into());
                    }
                }
                _ => {
                    return Err(format!(
                        "Unexpected exec state: {:?}. Check that the container is running.",
                        exec_state
                    )
                    .into());
                }
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
                cmd.as_deref().map(redact_injected),
                working_dir
            );

            Ok(self
                .client
                .create_exec(
                    container_id,
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

    // `tty`, and the command's exit code once it is done
    pub async fn tty_status(
        &self,
        reason: &str,
        container_id: &str,
        working_dir: Option<&str>,
        user: Option<&str>,
        cmd: Option<Vec<&str>>,
    ) -> Result<i64, AnyError> {
        let exec_id = self
            .create_exec(reason, container_id, working_dir, user, cmd, true)
            .await?;
        self.start_tty(&exec_id).await?;
        Ok(self
            .client
            .inspect_exec(&exec_id)
            .await?
            .exit_code
            .unwrap_or(-1))
    }

    // One non-interactive command with stdout and stderr passed through as they arrive; returns
    // its exit code.
    pub async fn passthrough(
        &self,
        container_id: &str,
        working_dir: Option<&str>,
        user: Option<&str>,
        cmd: Vec<&str>,
    ) -> Result<i64, AnyError> {
        use std::io::Write;

        let exec_id = self
            .create_exec("exec", container_id, working_dir, user, Some(cmd), false)
            .await?;
        let StartExecResults::Attached { mut output, .. } =
            self.client.start_exec(&exec_id, None).await?
        else {
            return Err("Could not start exec: the engine returned no attached stream. Check that the container is running.".into());
        };
        while let Some(item) = output.next().await {
            match item? {
                LogOutput::StdErr { message } => {
                    let mut e = std::io::stderr();
                    e.write_all(&message)?;
                    e.flush()?;
                }
                other => {
                    let mut o = std::io::stdout();
                    o.write_all(&other.into_bytes())?;
                    o.flush()?;
                }
            }
        }
        self.exit_code(&exec_id).await
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
            Err("Could not start exec: the engine returned no attached stream. Check that the container is running.".into())
        }
    }

    // `output` plus the exit code, for commands whose failure matters
    async fn output_with_code(
        &self,
        reason: &str,
        container_id: &str,
        user: Option<&str>,
        cmd: Option<Vec<&str>>,
    ) -> Result<(String, i64), AnyError> {
        let exec_id = self
            .create_exec(reason, container_id, None, user, cmd, false)
            .await?;
        let StartExecResults::Attached { output, .. } =
            self.client.start_exec(&exec_id, None).await?
        else {
            return Err("Could not start exec: the engine returned no attached stream. Check that the container is running.".into());
        };
        let data = collect(output).await?;
        Ok((data, self.exit_code(&exec_id).await?))
    }

    pub async fn status(
        &self,
        reason: &str,
        container_id: &str,
        user: Option<&str>,
        cmd: Option<Vec<&str>>,
    ) -> Result<i64, AnyError> {
        self.status_tagged(reason, container_id, user, cmd, None)
            .await
    }

    async fn status_tagged(
        &self,
        reason: &str,
        container_id: &str,
        user: Option<&str>,
        cmd: Option<Vec<&str>>,
        install: Option<(&str, &str)>,
    ) -> Result<i64, AnyError> {
        let exec_id = self
            .create_exec(reason, container_id, None, user, cmd, false)
            .await?;
        if let StartExecResults::Attached { output, .. } =
            self.client.start_exec(&exec_id, None).await?
        {
            log(output, install).await?;
        } else {
            return Err("Could not start exec: the engine returned no attached stream. Check that the container is running.".into());
        }

        self.exit_code(&exec_id).await
    }

    // The engine can report the stream closed a moment before it records the exit code; a
    // missing one is not a success.
    async fn exit_code(&self, exec_id: &str) -> Result<i64, AnyError> {
        for _ in 0..50 {
            let inspect = self.client.inspect_exec(exec_id).await?;
            if inspect.running != Some(true)
                && let Some(code) = inspect.exit_code
            {
                return Ok(code);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Err("The engine did not report an exit code for the command.".into())
    }

    pub async fn run(
        &self,
        reason: &str,
        container_id: &str,
        user: Option<&str>,
        cmd: Option<Vec<&str>>,
    ) -> Result<(), AnyError> {
        match self.status(reason, container_id, user, cmd).await? {
            0 => Ok(()),
            exit_code => {
                Err(format!("{}: exec failed with exit code {}", reason, exit_code).into())
            }
        }
    }

    // Steps run as the container's own user; `root: true` is the explicit opt-out. A
    // `skip_if` check runs first as the same user and a zero exit skips the step.
    pub async fn install(
        &self,
        container_name: &str,
        container_id: &str,
        steps: Vec<ResolvedStep>,
        checkpoints: Option<InstallCheckpoints<'_>>,
    ) -> Result<(), AnyError> {
        let names = steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>();
        live::install_begin(container_name, &names);
        match self
            .install_steps(container_name, container_id, &steps, checkpoints)
            .await
        {
            Ok(()) => {
                live::install_done(container_name);
                Ok(())
            }
            Err(e) => {
                live::install_failed(container_name);
                Err(e)
            }
        }
    }

    async fn install_steps(
        &self,
        container_name: &str,
        container_id: &str,
        steps: &[ResolvedStep],
        checkpoints: Option<InstallCheckpoints<'_>>,
    ) -> Result<(), AnyError> {
        let resumed = checkpoints.as_ref().map_or(0, |c| c.done);
        for (idx, step) in steps.iter().enumerate() {
            if idx < resumed {
                live::install_resumed(container_name, &step.name);
                continue;
            }
            let user = step.root.then_some(constants::ROOT_UID);
            let tag = (container_name, step.name.as_str());
            if let Some(check) = &step.skip_if {
                live::install_checking(container_name, &step.name);
                let check_cmd = inject(
                    &format!("#!/bin/sh\n{}", script_body(check)),
                    &format!("skip-{}.sh", idx),
                );
                let v = check_cmd.iter().map(|x| x.as_str()).collect::<Vec<_>>();
                if self
                    .status_tagged("install-check", container_id, user, Some(v), Some(tag))
                    .await?
                    == 0
                {
                    live::install_skipped(container_name, &step.name);
                    continue;
                }
            }
            live::install_step(container_name, &step.name);
            let cmd = format!("#!/bin/sh\nset -e\n{}", script_body(&step.run));
            let install_cmd = inject(cmd.as_str(), &format!("install-{}.sh", idx));
            let v = install_cmd.iter().map(|x| x.as_str()).collect::<Vec<_>>();
            match self
                .status_tagged("install", container_id, user, Some(v), Some(tag))
                .await?
            {
                0 => {
                    live::install_step_done(container_name);
                    if let Some(c) = &checkpoints {
                        c.checkpoints
                            .save(self.client, container_id, steps, idx + 1)
                            .await;
                    }
                }
                code => {
                    return Err(format!(
                        "install step '{}' failed: exec failed with exit code {}",
                        step.name, code
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    pub async fn chown(&self, container_id: &str, uid: &i32, dir: &str) -> Result<(), AnyError> {
        self.chown_with(container_id, uid, dir, true).await
    }

    // The directory itself, not what is in it: a cache volume's root belongs to root until
    // someone hands it over, and what is in it is already its owner's.
    pub async fn chown_dir(
        &self,
        container_id: &str,
        uid: &i32,
        dir: &str,
    ) -> Result<(), AnyError> {
        self.chown_with(container_id, uid, dir, false).await
    }

    async fn chown_with(
        &self,
        container_id: &str,
        uid: &i32,
        dir: &str,
        recursive: bool,
    ) -> Result<(), AnyError> {
        // TODO: this is true if the volume was first mounted as the right user
        // otherwise chown is still needed. I could probably check if a setup container had run first
        // because this is the main reason the volumes would be owned by root

        // if let ContainerEngine::Podman = self.backend.engine {
        //    log::debug!("Podman won't need chown. Skipping");
        //    return Ok(());
        //};

        log::debug!("Changing ownership... ({} {})", uid, dir);

        let uid_format = format!("{}:{}", uid, uid);
        let flags = if recursive { "-R " } else { "" };
        let (chown_response, chown_code) = self
            .output_with_code(
                "chown",
                container_id,
                Some(constants::ROOT_USER),
                Some(vec![
                    "sh",
                    "-c",
                    &format!("chown {}{} {}", flags, uid_format, chown_target(dir)),
                ]),
            )
            .await?;

        log::debug!("{}", chown_response);
        if chown_code != 0 {
            eprintln!(
                "WARN: could not change ownership of {} to {} (exit {}): {}",
                dir,
                uid_format,
                chown_code,
                chown_response.trim()
            );
        }
        Ok(())
    }

    pub async fn default_uid(&self, container_id: &str) -> Result<i32, AnyError> {
        let output = self
            .output("default-uid", container_id, None, Some(vec!["id", "-u"]))
            .await?;
        Ok(output.trim().parse::<i32>()?)
    }

    pub async fn ensure_user(&self, container_id: &str) -> Result<(), AnyError> {
        self.ensure_user_with(container_id, true).await
    }

    // For a container that works without the account, a failure is only worth a debug line
    pub async fn ensure_user_quietly(&self, container_id: &str) -> Result<(), AnyError> {
        self.ensure_user_with(container_id, false).await
    }

    async fn ensure_user_with(&self, container_id: &str, warn: bool) -> Result<(), AnyError> {
        let ensure_user_cmd = inject(
            r#"exec 2>&1
                       grep -q "^$ROOZ_META_USER:x:$ROOZ_META_UID:" /etc/passwd && exit 0
                       sed -i "/:x:${ROOZ_META_UID}:/d" /etc/passwd && \
                       echo "$ROOZ_META_USER:x:$ROOZ_META_UID:$ROOZ_META_UID:$ROOZ_META_USER:$ROOZ_META_HOME:/bin/sh" >> /etc/passwd"#.to_string()
            .as_ref(),
            "make_user.sh",
        );

        let (ensure_user_output, ensure_user_code) = self
            .output_with_code(
                "ensure-user",
                container_id,
                Some(constants::ROOT_UID),
                Some(ensure_user_cmd.iter().map(String::as_str).collect()),
            )
            .await?;
        log::debug!("{}", ensure_user_output);
        // images without a writable /etc/passwd (distroless, scratch-based) have no account to
        // manage, and everything but the user's name works without one
        if ensure_user_code != 0 && warn {
            eprintln!(
                "WARN: could not create the container user (exit {}): {}",
                ensure_user_code,
                ensure_user_output.trim()
            );
        }
        Ok(())
    }
}

fn script_body(script: &str) -> &str {
    if script.starts_with("#!") {
        script.split_once('\n').map(|x| x.1).unwrap_or(script)
    } else {
        script
    }
}

// chown targets are mount target paths from workspace config, so they reach the shell
// as untrusted strings. `~` is not a literal here - it stands for the container user's
// home - so every literal segment around it is quoted separately and the expansion
// itself stays quoted: a hostile home value cannot be split or re-parsed either.
fn chown_target(dir: &str) -> String {
    dir.split('~')
        .map(sh::quote)
        .collect::<Vec<_>>()
        .join("\"${ROOZ_META_HOME}\"")
}

#[cfg(test)]
mod tests {
    use super::chown_target;

    #[test]
    fn an_unterminated_run_is_kept_bounded() {
        use super::{MAX_PENDING, trim_unterminated};
        // a progress meter: only what follows the last carriage return is kept
        let mut pending = Vec::new();
        for i in 0..100_000 {
            pending.extend_from_slice(format!("\r{}%", i).as_bytes());
            trim_unterminated(&mut pending);
        }
        assert!(pending.len() <= MAX_PENDING + 16);
        // a run with no carriage return at all: capped to the last MAX_PENDING bytes
        let mut pending = vec![b'x'; MAX_PENDING * 4];
        trim_unterminated(&mut pending);
        assert_eq!(pending.len(), MAX_PENDING);
    }

    #[test]
    fn plain_paths_are_single_quoted() {
        assert_eq!(chown_target("/work"), "'/work'");
    }

    #[test]
    fn tilde_becomes_a_quoted_home_expansion() {
        assert_eq!(chown_target("~/.nuget"), "''\"${ROOZ_META_HOME}\"'/.nuget'");
        assert_eq!(chown_target("~"), "''\"${ROOZ_META_HOME}\"''");
        assert_eq!(chown_target("/a~b"), "'/a'\"${ROOZ_META_HOME}\"'b'");
    }

    #[test]
    fn hostile_mount_targets_stay_one_argument() {
        // a mount target straight from an untrusted config
        let hostile = "/work/x; cat /home/rooz_user/.ssh/id_ed25519 > /work/LEAKED_KEY";
        assert_eq!(chown_target(hostile), format!("'{}'", hostile));
    }

    #[test]
    fn shell_metacharacters_are_never_reachable() {
        for hostile in [
            "x'; touch /tmp/pwned; #",
            "a && cat /etc/passwd",
            "$(id -u)",
            "`id -u`",
            "a|b;c&d>e<f",
            "line\nbreak",
            "back\\slash",
        ] {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf %s {}", chown_target(hostile)))
                .output()
                .unwrap();
            assert!(out.status.success());
            assert_eq!(
                String::from_utf8(out.stdout).unwrap(),
                hostile,
                "not neutralized: {}",
                hostile
            );
        }

        // the home expansion must not be re-evaluated
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "ROOZ_META_HOME='$(echo pwned) $HOME'; printf %s {}",
                chown_target("~/.nuget")
            ))
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            "$(echo pwned) $HOME/.nuget"
        );
    }
}
