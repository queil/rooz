use crate::{
    api::WorkspaceApi,
    cli::{WorkEnvParams, WorkParams},
    config::config::{ConfigSource, FileFormat, RoozCfg},
    model::{types::AnyError, volume::WORK_ROLE},
    util::labels::{self, Labels},
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

        let identity = self.read_age_identity().await?;

        if let Some(labels) = &container.labels {
            if interactive {}

            let config_source = &labels[labels::CONFIG_ORIGIN];
            let format = FileFormat::from_path(config_source);
            let original_body = &labels[labels::CONFIG_BODY];
            let mut original_config = RoozCfg::deserialize_config(original_body, format)?.unwrap();

            let config_to_apply = if interactive {
                original_config.decrypt(&identity).await?;

                let decrypted_string = original_config.to_string(format)?;
                let (encrypted_config, _) = self
                    .edit_config_core(decrypted_string.clone(), format, &identity)
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
