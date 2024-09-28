use std::process::{Command, Stdio};

use crate::{
    api::WorkspaceApi,
    constants,
    labels::{self, Labels},
    model::{config::FinalCfg, types::AnyError, volume::RoozVolume},
};

impl<'a> WorkspaceApi<'a> {
    pub async fn attach_vscode(&self, workspace_key: &str) -> Result<(), AnyError> {
        self.start_workspace(workspace_key).await?;

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
        let enter_labels = Labels::new(Some(workspace_key), None)
            .with_container(container_id.or(Some(constants::DEFAULT_CONTAINER_NAME)));

        let container = self
            .api
            .container
            .get_single(&enter_labels)
            .await?
            .ok_or(format!("Workspace not found: {}", &workspace_key))?;

        println!("{}", termion::clear::All);

        let mut shell_value = vec![constants::DEFAULT_SHELL.to_string()];

        if let Some(labels) = &container.labels {
            if labels.contains_key(labels::RUNTIME_CONFIG) {
                shell_value = FinalCfg::from_string(labels[labels::RUNTIME_CONFIG].clone())?.shell;
            }
        }

        if let Some(shell) = shell {
            shell_value = shell.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        }

        let container_id = container.id.as_deref().unwrap();

        self.start_workspace(workspace_key).await?;

        if !root {
            self.api.exec.ensure_user(container_id).await?;
            for v in &volumes {
                self.api
                    .exec
                    .chown(&container_id, chown_uid, &v.path)
                    .await?;
            }
        }

        self.api
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
            .await?;

        if ephemeral {
            self.api.container.kill(&container_id).await?;
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
