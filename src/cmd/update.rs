use std::str::FromStr;
use std::{fs, process::exit};

use crate::{
    api::WorkspaceApi,
    cli::{WorkEnvParams, WorkParams},
    cmd::plan::PlanMode,
    config::{
        config::{ConfigPath, ConfigSource, ConfigType, FileFormat, RoozCfg},
        runtime::{RuntimeConfig, SECRET_MASK},
    },
    model::types::AnyError,
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
        dry_run: bool,
        allow_secrets: Option<bool>,
    ) -> Result<(), AnyError> {
        let config_source = self
            .stored_config_source(workspace_key, interactive, true)
            .await?;

        let git_ssh_url = match &config_source {
            Some(ConfigSource::Update { value, .. }) if value.git_ssh_url.is_some() => None,
            _ => self.stored_git_ssh_url(workspace_key).await?,
        };

        if dry_run {
            let plan = self
                .plan_new(
                    workspace_key,
                    &WorkParams {
                        git_ssh_url,
                        env: spec.clone(),
                        pull_image: !no_pull,
                        allow_secrets,
                        ..Default::default()
                    },
                    config_source,
                    PlanMode::Update {
                        purge: matches!(mode, UpdateMode::Purge),
                    },
                )
                .await?;
            println!("{}", plan);
            return Ok(());
        }

        self.api.volume.require_ssh_key().await?;

        // pull before anything is removed: a failed pull must leave the workspace as it was. Only
        // an image named by the config or the environment is known here; one that comes from
        // 'bases' is still pulled by 'new'.
        let mut pull_image = !(no_pull || interactive);
        if pull_image {
            let image = spec.image.clone().or_else(|| match &config_source {
                Some(ConfigSource::Update { value, .. }) => value.image.clone(),
                _ => None,
            });
            if let Some(image) = image {
                self.api.image.ensure(&image, true).await?;
                pull_image = false;
            }
        }

        match mode {
            UpdateMode::Apply => self.remove_containers_only(workspace_key, true).await?,
            UpdateMode::Purge => self.remove(workspace_key, true, true).await?,
        };

        self.new(
            workspace_key,
            &WorkParams {
                git_ssh_url,
                env: spec.clone(),
                pull_image,
                allow_secrets,
                ..Default::default()
            },
            config_source,
            false,
            true,
        )
        .await?;

        Ok(())
    }

    // The configuration a workspace is rebuilt from. `from_origin` re-reads it from where it came
    // from (what 'update' does); otherwise the body stored with the workspace is used as is.
    pub(crate) async fn stored_config_source(
        &self,
        workspace_key: &str,
        interactive: bool,
        from_origin: bool,
    ) -> Result<Option<ConfigSource>, AnyError> {
        let labels = Labels::workspace_config(workspace_key);

        let volume = self
            .api
            .volume
            .get_single(&labels)
            .await?
            .ok_or(format!("Workspace not found: {}", workspace_key))?;

        let config_source = {
            if let Some(config_source) = &volume.labels.get(labels::CONFIG_ORIGIN) {
                let format = FileFormat::from_path(config_source)?;
                let config_path = ConfigPath::from_str(config_source)?;
                let mut original_body = self.config.read(workspace_key, &ConfigType::Body).await?;
                let mut pre_merged: Option<RoozCfg> = None;

                if !interactive && from_origin {
                    match &config_path {
                        ConfigPath::File { path } => {
                            original_body = fs::read_to_string(path)?;
                        }
                        ConfigPath::Git { url, file_path } => {
                            let clone_env = CloneEnv {
                                workspace_key: workspace_key.to_string(),
                                ..Default::default()
                            };

                            let (result, _) = self
                                .git
                                .clone_config_repo(clone_env, url, file_path)
                                .await?;
                            if let Some(cb) = result {
                                original_body = cb.body;
                                pre_merged = cb.merged;
                            };
                        }
                    };
                }

                let mut original_config = match pre_merged {
                    Some(merged) => merged,
                    None => RoozCfg::deserialize_config(&original_body, format)?.unwrap(),
                };

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
                Some(ConfigSource::Update {
                    value: config_to_apply,
                    origin: config_source.to_string(),
                    format,
                })
            } else {
                if interactive {
                    eprintln!(
                        "WARN: The --tweak flag has no effect on workspaces without config. Please remove it to update the workspace"
                    );
                    exit(1)
                }
                None
            }
        };
        Ok(config_source)
    }

    pub(crate) async fn stored_git_ssh_url(
        &self,
        workspace_key: &str,
    ) -> Result<Option<String>, AnyError> {
        let body = self
            .config
            .read(workspace_key, &ConfigType::Runtime)
            .await?;
        if body.trim().is_empty() {
            return Ok(None);
        }
        match RuntimeConfig::from_string(body)?.git_ssh_url {
            Some(url) if url.contains(SECRET_MASK) => {
                eprintln!(
                    "WARN: the workspace's repository URL contains a secret and was not persisted; \
                     the repository is not re-cloned"
                );
                Ok(None)
            }
            url => Ok(url),
        }
    }
}
