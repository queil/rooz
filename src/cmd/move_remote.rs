use std::str::FromStr;
use std::{path::PathBuf, process::Stdio};

use futures::StreamExt;
use tokio::{io::AsyncWriteExt, process::Command};

use crate::{
    api::WorkspaceApi,
    config::{config::ConfigType, remotes, runtime::RuntimeConfig},
    model::types::AnyError,
    util::{
        id,
        labels::{CONFIG_ORIGIN, Labels},
        progress,
    },
};

// The receiving side is another rooz process pointed at the target host with -H, so each engine
// is only ever driven by a complete, ordinary client. The workspace's data streams from this
// process's engine into that process's stdin, through the operator's machine, never buffered
// whole.
fn on_host(host: &str, args: &[&str]) -> Result<Command, AnyError> {
    let exe: PathBuf = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.args(["-H", host]).args(args);
    Ok(cmd)
}

async fn captured(host: &str, args: &[&str]) -> Result<String, AnyError> {
    let out = on_host(host, args)?.stdin(Stdio::null()).output().await?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .trim_start_matches("Error:")
            .trim()
            .to_string()
            .into())
    }
}

async fn inherited(host: &str, args: &[String]) -> Result<(), AnyError> {
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let status = on_host(host, &args)?.stdin(Stdio::null()).status().await?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("'rooz -H {} {}' failed ({})", host, args.join(" "), status).into())
    }
}

impl<'a> WorkspaceApi<'a> {
    // The arguments that make 'new' on the target build this workspace: its config file where
    // it has one, otherwise what its flags set, which the runtime config remembers.
    async fn remote_new_args(
        &self,
        from: &str,
        to: &str,
        allow_secrets: Option<bool>,
    ) -> Result<Vec<String>, AnyError> {
        let volume = self
            .api
            .volume
            .get_single(&Labels::workspace_config(from))
            .await?
            .ok_or(format!("Workspace not found: {}", from))?;

        let mut args = vec!["new".to_string(), to.to_string()];
        match volume.labels.get(CONFIG_ORIGIN) {
            Some(origin) => {
                if let Ok(crate::config::config::ConfigPath::File { path }) =
                    crate::config::config::ConfigPath::from_str(origin)
                    && !std::path::Path::new(&path).is_file()
                {
                    return Err(format!(
                        "The workspace was made from '{}', which is not on this machine any \
                             more. The target is rebuilt from the config file.",
                        path
                    )
                    .into());
                }
                args.extend(["--config".to_string(), origin.clone()]);
            }
            None => {
                let rt = RuntimeConfig::from_string(
                    self.config.read(from, &ConfigType::Runtime).await?,
                )?;
                args.extend(["--image".into(), rt.image, "--user".into(), rt.user]);
                args.extend(["--uid".into(), rt.uid.to_string()]);
                if !rt.caches.is_empty() {
                    args.extend(["--caches".into(), rt.caches.join(",")]);
                }
                args.extend(["--privileged".into(), rt.privileged.to_string()]);
            }
        }
        if let Some(url) = self.stored_git_ssh_url(from).await? {
            args.extend(["--git-ssh-url".into(), url]);
        }
        if let Some(allow) = allow_secrets {
            args.extend(["--allow-secrets".into(), allow.to_string()]);
        }
        Ok(args)
    }

    async fn send_volume(
        &self,
        host: &str,
        from: &str,
        to: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<(), AnyError> {
        let mut args = vec![
            "internal".to_string(),
            "receive-volume".into(),
            to.to_string(),
        ];
        for (k, v) in labels {
            args.extend(["--label".to_string(), format!("{}={}", k, v)]);
        }
        let args_ref = args.iter().map(String::as_str).collect::<Vec<_>>();
        let mut receiver = on_host(host, &args_ref)?
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut sink = receiver.stdin.take().ok_or("receiver has no stdin")?;

        // tar runs in the container, keeping owners and modes; the target's upload keeps them too
        let archive_container = self
            .api
            .container
            .volume_tar_container(from, "/from")
            .await?;
        let sent: Result<(), AnyError> = async {
            let (exec_id, tar) = self
                .api
                .exec
                .stdout_stream(
                    "move-copy",
                    &archive_container,
                    vec!["tar", "-C", "/from", "-cf", "-", "."],
                )
                .await?;
            let mut tar = std::pin::pin!(tar);
            while let Some(chunk) = tar.next().await {
                sink.write_all(&chunk?).await?;
            }
            match self.api.exec.exit_code(&exec_id).await? {
                0 => Ok(()),
                code => Err(format!("tar exited with {}", code).into()),
            }
        }
        .await;
        drop(sink);
        self.api
            .container
            .remove(&archive_container, true)
            .await
            .ok();

        let status = receiver.wait().await?;
        match (sent, status.success()) {
            (Ok(()), true) => Ok(()),
            (Err(e), _) => Err(format!("Sending volume '{}' failed: {}", from, e).into()),
            (Ok(()), false) => {
                Err(format!("The target could not receive volume '{}' ({})", to, status).into())
            }
        }
    }

    pub async fn move_to_host(
        &self,
        from: &str,
        to: &str,
        host: &str,
        current_host: Option<&str>,
        remove_source: bool,
        dry_run: bool,
        allow_secrets: Option<bool>,
    ) -> Result<(), AnyError> {
        id::validate_workspace_name(to)?;
        if current_host == Some(host) {
            return Err(format!(
                "The workspace is already on host '{}'. Use 'rooz move <name> <new-name>' to rename it.",
                host
            )
            .into());
        }
        remotes::connected_socket(host)?;

        self.api
            .volume
            .get_single(&Labels::workspace_config(from))
            .await?
            .ok_or(format!("Workspace not found: {}", from))?;

        let moves = self.volumes_to_move(from, to).await?;
        let new_args = self.remote_new_args(from, to, allow_secrets).await?;

        let platform = captured(host, &["internal", "preflight", to]).await?;
        if platform != self.api.container.backend.platform {
            eprintln!(
                "WARN: the target runs {} and this engine {}: compiled artifacts in the work \
                 volume will not run there.",
                platform, self.api.container.backend.platform
            );
        }

        if dry_run {
            println!("Volumes to copy to '{}':", host);
            for m in &moves {
                println!("  {} -> {}", m.from, m.to);
            }
            println!();
            let mut plan = new_args.clone();
            plan.push("--dry-run".into());
            inherited(host, &plan).await?;
            println!(
                "The source stays stopped{}.",
                if remove_source {
                    " and is then removed"
                } else {
                    ""
                }
            );
            return Ok(());
        }

        // checked on the target before anything is copied: the same gates a run applies
        let mut gate = new_args.clone();
        gate.push("--dry-run".into());
        inherited(host, &gate).await?;

        let was_running = !self
            .api
            .container
            .get_running(&Labels::from(&[Labels::workspace(from)]))
            .await?
            .is_empty();
        self.stop(from).await?;

        let moved: Result<(), AnyError> = async {
            for m in &moves {
                progress::phase("copy", format!("{} -> {} on {}", m.from, m.to, host));
                self.send_volume(host, &m.from, &m.to, &m.labels).await?;
            }
            let mut build = new_args.clone();
            build.extend(["--start".to_string(), "false".to_string()]);
            inherited(host, &build).await
        }
        .await;

        if let Err(e) = moved {
            if let Err(cleanup) = inherited(host, &["rm".into(), to.into(), "--force".into()]).await
            {
                eprintln!(
                    "WARN: could not remove the partial workspace '{}' on '{}': {}. Remove it with \
                     'rooz -H {} rm {} --force'.",
                    to, host, cleanup, host, to
                );
            }
            if was_running {
                self.start(from).await.ok();
            }
            return Err(format!("Move failed; '{}' is untouched here. {}", from, e).into());
        }

        if remove_source {
            self.remove(from, false, true).await?;
            println!(
                "Moved '{}' to '{}' on '{}' and removed the source.",
                from, to, host
            );
        } else {
            println!(
                "Moved '{}' to '{}' on '{}'. The source is stopped; remove it with 'rooz rm {}' when satisfied.",
                from, to, host, from
            );
        }
        Ok(())
    }
}
