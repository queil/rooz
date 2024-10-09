use crate::{
    api::WorkspaceApi,
    cli::{WorkEnvParams, WorkParams},
    config::config::{ConfigPath, ConfigSource, FileFormat, RoozCfg},
    constants,
    model::{types::AnyError, volume::WORK_ROLE},
    util::{
        git::CloneEnv,
        labels::{self, Labels},
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
        let labels = Labels::new(Some(workspace_key), Some(WORK_ROLE));

        let container = self
            .api
            .container
            .get_single(&labels)
            .await?
            .ok_or(format!("Workspace not found: {}", &workspace_key))?;

        match mode {
            UpdateMode::Apply => self.remove_containers_only(&workspace_key, true).await?,
            UpdateMode::Purge => self.remove(&workspace_key, true).await?,
        };

        let identity = self.crypt.read_age_identity().await?;

        if let Some(labels) = &container.labels {
            let config_source = &labels[labels::CONFIG_ORIGIN];
            let format = FileFormat::from_path(config_source);
            let mut original_body = labels[labels::CONFIG_BODY].clone();

            match ConfigPath::from_str(&config_source)? {
                ConfigPath::File { .. } => (),
                ConfigPath::Git { url, file_path } => {
                    let clone_env = CloneEnv {
                        image: constants::DEFAULT_IMAGE.into(),
                        uid: constants::DEFAULT_UID.to_string(),
                        workspace_key: workspace_key.to_string(),
                        working_dir: constants::WORK_DIR.to_string(),
                        use_volume: false,
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

            let mut original_config = RoozCfg::deserialize_config(&original_body, format)?.unwrap();

            let config_to_apply = if interactive {
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

            self.new(
                &labels[labels::WORKSPACE_KEY],
                &WorkParams {
                    env: spec.clone(),
                    pull_image: if no_pull { false } else { true },
                    ..Default::default()
                },
                Some(ConfigSource::Body {
                    value: config_to_apply,
                    origin: config_source.to_string(),
                    format,
                }),
                false,
                &identity,
            )
            .await?;
        }
        Ok(())
    }
}
