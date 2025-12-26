use std::{fs, process::exit};

use crate::{
    api::WorkspaceApi,
    cli::{WorkEnvParams, WorkParams},
    config::config::{ConfigPath, ConfigSource, ConfigType, FileFormat, RoozCfg},
    model::types::AnyError,
    util::{
        git::CloneEnv,
        labels::{self, Labels, WORKSPACE_CONFIG_ROLE},
    },
};

pub enum UpdateMode {
    Apply,
    Purge,
}

impl<'a> WorkspaceApi<'a> {
    pub async fn update(
        &self,
        workspace_key: &str,
        spec: &WorkEnvParams,
        interactive: bool,
        mode: UpdateMode,
        no_pull: bool,
    ) -> Result<(), AnyError> {
        let labels = Labels::from(&[
            Labels::workspace(workspace_key),
            Labels::role(WORKSPACE_CONFIG_ROLE),
        ]);

        let volume = self
            .api
            .volume
            .get_single(&labels)
            .await?
            .ok_or(format!("Workspace not found: {}", &workspace_key))?;

        let (config_path, config_source) = {
            if let Some(config_source) = &volume.labels.get(labels::CONFIG_ORIGIN) {
                let format = FileFormat::from_path(config_source);
                let config_path = ConfigPath::from_str(&config_source)?;
                let mut original_body = self.config.read(workspace_key, &ConfigType::Body).await?;

                if !interactive {
                    match &config_path {
                        ConfigPath::File { path } => {
                            original_body = fs::read_to_string(&path)?;
                        }
                        ConfigPath::Git { url, file_path } => {
                            let clone_env = CloneEnv {
                                workspace_key: workspace_key.to_string(),
                                use_volume: false,
                                depth_override: Some(1),
                                ..Default::default()
                            };

                            match self
                                .git
                                .clone_config_repo(clone_env, &url, &file_path)
                                .await?
                            {
                                Some(cfg) => original_body = cfg.to_string(),
                                None => (),
                            };
                        }
                    };
                }

                let mut original_config =
                    RoozCfg::deserialize_config(&original_body, format)?.unwrap();

                let config_to_apply = if interactive {
                    let identity = self.api.get_system_config().await?.age_identity()?;
                    self.config.decrypt(&mut original_config, &identity).await?;

                    let decrypted_string = original_config.to_string(format)?;
                    let (encrypted_config, _) = self
                        .config
                        .edit_string(decrypted_string.clone(), format, &identity)
                        .await?;
                    encrypted_config
                } else {
                    original_config
                };
                (
                    Some(config_path),
                    Some(ConfigSource::Update {
                        value: config_to_apply,
                        origin: config_source.to_string(),
                        format,
                    }),
                )
            } else {
                if interactive {
                    eprintln!(
                        "WARN: The --tweak flag has no effect on workspaces without config. Please remove it to update the workspace"
                    );
                    exit(1)
                }
                (None, None)
            }
        };

        match mode {
            UpdateMode::Apply => self.remove_containers_only(&workspace_key, true).await?,
            UpdateMode::Purge => self.remove(&workspace_key, true, true).await?,
        };

        self.new(
            &volume.labels[labels::WORKSPACE_KEY],
            &WorkParams {
                git_ssh_url: config_path
                    .map(|c| match &c {
                        ConfigPath::Git { url, .. } if c.is_in_repo() => Some(url.to_string()),
                        _ => None,
                    })
                    .flatten(),
                env: spec.clone(),
                pull_image: if no_pull || interactive { false } else { true },
                ..Default::default()
            },
            config_source,
            false,
        )
        .await?;

        Ok(())
    }
}
