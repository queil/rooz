use std::{
    io::stdout,
    process::{Command, Stdio},
    thread::sleep,
    time::Duration,
};

use crossterm::{
    execute,
    terminal::{Clear, ClearType},
};

use crate::{
    api::WorkspaceApi,
    config::{config::ConfigType, runtime::RuntimeConfig},
    constants::{self},
    model::{types::AnyError, volume::RoozVolume},
    util::labels::Labels,
};

impl<'a> WorkspaceApi<'a> {
    pub async fn attach_vscode(&self, workspace_key: &str) -> Result<(), AnyError> {
        self.start(workspace_key).await?;

        let hex = format!(r#"{{"containerName":"{}"}}"#, workspace_key)
            .as_bytes()
            .iter()
            .map(|&b| format!("{:02x}", b))
            .collect::<Vec<String>>()
            .join("");
        let mut command = Command::new("code");
        command.arg("--folder-uri");
        command.arg(format!("vscode-remote://attached-container+{}/work", hex));
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        match command.spawn() {
            Ok(_) => Ok(()),
            Err(e) => Err(Box::new(e)),
        }
    }

    pub async fn enter(
        &self,
        workspace_key: &str,
        working_dir: Option<&str>,
        shell: Option<Vec<&str>>,
        container_id: Option<&str>,
        volumes: Vec<RoozVolume>,
        chown_uid: &str,
        root: bool,
        ephemeral: bool,
    ) -> Result<(), AnyError> {
        let enter_labels = Labels::from(&[
            Labels::workspace(workspace_key),
            Labels::container(container_id.unwrap_or(constants::WORK_CONTAINER_NAME)),
        ]);

        let container = self
            .api
            .container
            .get_single(&enter_labels)
            .await?
            .ok_or(format!("Workspace not found: {}", &workspace_key))?;

        let config = self
            .config
            .read(workspace_key, &ConfigType::Runtime)
            .await?;

        let mut shell_value = RuntimeConfig::from_string(config)?.shell;

        if let Some(shell) = shell {
            shell_value = shell.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        }

        let container_id = container.id.as_deref().unwrap();

        // the loop here is needed for auto-reconnecting the session
        loop {
            execute!(stdout(), Clear(ClearType::All))?;
            match self.start(workspace_key).await {
                Ok(_) => (),
                Err(e) => {
                    log::debug!("{}", e);
                    eprintln!("Rooz is reconnecting to {}", workspace_key);
                    sleep(Duration::from_millis(2_000));
                    continue;
                }
            };

            if !root {
                self.api.exec.ensure_user(container_id).await?;
                for v in &volumes {
                    self.api
                        .exec
                        .chown(&container_id, chown_uid, &v.path)
                        .await?;
                }
            }

            match self
                .api
                .exec
                .tty(
                    "work",
                    &container_id,
                    true,
                    working_dir,
                    if root {
                        Some(constants::ROOT_USER)
                    } else {
                        None
                    },
                    Some(shell_value.iter().map(|v| v.as_str()).collect::<Vec<_>>()),
                )
                .await
            {
                Ok(_) => break,
                Err(e) => {
                    log::debug!("{}", e);
                }
            };
        }
        if ephemeral {
            self.api.container.kill(&container_id, true).await?;
            for vol in volumes.iter().filter(|v| v.is_exclusive()) {
                self.api
                    .volume
                    .remove_volume(&vol.safe_volume_name(), true)
                    .await?;
            }
        }
        Ok(())
    }
}
