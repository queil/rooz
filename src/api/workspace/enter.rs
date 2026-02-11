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
    model::types::AnyError,
    util::labels::Labels,
};

impl<'a> WorkspaceApi<'a> {
    pub async fn attach_vscode(&self, workspace_key: &str) -> Result<(), AnyError> {
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
        container_name: Option<&str>,
        chown_uid: &str,
        root: bool,
    ) -> Result<String, AnyError> {
        let container_name = container_name.unwrap_or(constants::DEFAULT_CONTAINER_NAME);
        let enter_labels = Labels::from(&[
            Labels::workspace(workspace_key),
            Labels::container(container_name),
        ]);

        let container = self
            .api
            .container
            .get_single(&enter_labels)
            .await?
            .ok_or(format!("Workspace not found: {}", &workspace_key))?;

        let config = RuntimeConfig::from_string(
            self.config
                .read(workspace_key, &ConfigType::Runtime)
                .await?,
        )?;

        let mut shell_value = config.shell.clone();

        if let Some(shell) = shell {
            shell_value = shell.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        }

        let container_id = container.id.as_deref().unwrap();

        // the loop here is needed for auto-reconnecting the session
        loop {
            execute!(stdout(), Clear(ClearType::All))?;
            match self.start(workspace_key, Some((&config, chown_uid))).await {
                Ok(_) => (),
                Err(e) => {
                    log::debug!("{}", e);
                    eprintln!("Rooz is reconnecting to {}", workspace_key);
                    sleep(Duration::from_millis(2_000));
                    continue;
                }
            };

            //TODO: v2 chown is disabled for sidecars
            // symlinking might be happening in wrapped entrypoints so maybe chowning could to
            // Chowing as exec to work in non-root containers
            if !root && container_name == constants::DEFAULT_CONTAINER_NAME {
                self.api.exec.ensure_user(container_id).await?;

                for (target, _) in &config.real_mounts {
                    self.api
                        .exec
                        .chown(&container_id, chown_uid, target.as_str())
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
        Ok(container_id.to_string())
    }
}
