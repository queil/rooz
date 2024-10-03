use std::{
    fs::{self},
    io,
};

use crate::{
    api::WorkspaceApi,
    cli::{ConfigFormat, ConfigPart, WorkEnvParams, WorkParams, WorkspacePersistence},
    config::{
        config::{ConfigSource, FileFormat, RoozCfg},
        runtime::RuntimeConfig,
    },
    model::{types::AnyError, volume::WORK_ROLE},
    util::labels::{self, Labels},
};

use colored::Colorize;

impl<'a> WorkspaceApi<'a> {
    pub async fn show_config(
        &self,
        workspace_key: &str,
        part: ConfigPart,
        output: Option<ConfigFormat>,
    ) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), Some(WORK_ROLE));

        let container = self
            .api
            .container
            .get_single(&labels)
            .await?
            .ok_or(format!("Workspace not found: {}", &workspace_key))?;

        if let Some(labels) = container.labels {
            let content: Option<String> = match part {
                ConfigPart::Origin => labels.get(labels::CONFIG_ORIGIN).cloned(),
                ConfigPart::Body => {
                    let new_format = output.map(|c| match c {
                        ConfigFormat::Toml => FileFormat::Toml,
                        ConfigFormat::Yaml => FileFormat::Yaml,
                    });

                    let maybe_body = labels.get(labels::CONFIG_BODY);
                    if let Some(body) = maybe_body {
                        if let Some(format) = new_format {
                            let origin_path = labels.get(labels::CONFIG_ORIGIN).unwrap();
                            let original_format = FileFormat::from_path(&origin_path);
                            let cfg = RoozCfg::from_string(&body, original_format)?;
                            Some(cfg.to_string(format)?.to_string())
                        } else {
                            Some(body.to_string())
                        }
                    } else {
                        None
                    }
                }
                ConfigPart::Runtime => {
                    if let Some(runtime_config) = labels.get(labels::RUNTIME_CONFIG) {
                        match output {
                            Some(ConfigFormat::Yaml) => {
                                let cfg = RuntimeConfig::from_string(runtime_config.to_string())?;
                                Some(serde_yaml::to_string(&cfg)?)
                            }
                            _ => Some(runtime_config.to_string()),
                        }
                    } else {
                        None
                    }
                }
            };

            println!("{}", content.unwrap_or("N/A".to_string()))
        }
        Ok(())
    }

    fn edit_error(&self, message: &str) -> () {
        eprintln!("{}", "Error: Invalid configuration".bold().red());
        eprintln!("{}", message.red());
        eprintln!("Press any key to continue editing...");
        io::stdin().read_line(&mut String::new()).unwrap();
    }

    async fn edit_config_core(
        &self,
        body: String,
        format: FileFormat,
    ) -> Result<(RoozCfg, String), AnyError> {
        let mut edited_body = body;
        let mut edited_config;
        loop {
            edited_body = match edit::edit(edited_body.clone()) {
                Ok(b) => b,
                Err(err) => {
                    self.edit_error(&err.to_string());
                    continue;
                }
            };
            edited_config = match RoozCfg::from_string(&edited_body, format) {
                Ok(c) => c,
                Err(err) => {
                    self.edit_error(&err.to_string());
                    continue;
                }
            };

            match (&edited_config.vars, &edited_config.secrets) {
                (Some(vars), Some(secrets)) => {
                    if let Some(duplicate_key) =
                        vars.keys().find(|k| secrets.contains_key(&k.to_string()))
                    {
                        self.edit_error(&format!(
                            "The key: '{}' can be only defined in either vars or secrets.",
                            &duplicate_key.to_string()
                        ));
                        continue;
                    }
                }
                _ => (),
            };
            break;
        }
        let identity = self.read_age_identity().await?;

        edited_config.encrypt(identity).await?;
        Ok((edited_config, edited_body))
    }

    pub async fn edit_existing(
        &self,
        workspace_key: &str,
        spec: &WorkEnvParams,
    ) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), Some(WORK_ROLE));

        let container = self
            .api
            .container
            .get_single(&labels)
            .await?
            .ok_or(format!("Workspace not found: {}", &workspace_key))?;

        if let Some(labels) = &container.labels {
            let config_source = &labels[labels::CONFIG_ORIGIN];
            let format = FileFormat::from_path(config_source);
            let mut config =
                RoozCfg::deserialize_config(&labels[labels::CONFIG_BODY], format)?.unwrap();
            config.decrypt(self.read_age_identity().await?).await?;
            let decrypted_string = config.to_string(format)?;
            let (encrypted_config, edited_string) = self
                .edit_config_core(decrypted_string.clone(), format)
                .await?;

            //TODO: this check should be performed on the fully constructed config (to pick up changes in e.g. ROOZ_ env vars)
            if edited_string != decrypted_string {
                self.new(
                    &WorkParams {
                        env: spec.clone(),
                        ..Default::default()
                    },
                    Some(ConfigSource::Body {
                        value: encrypted_config,
                        origin: config_source.to_string(),
                        format,
                    }),
                    Some(WorkspacePersistence {
                        name: labels[labels::WORKSPACE_KEY].to_string(),
                        replace: false,
                        apply: true,
                    }),
                )
                .await?;
            }
        }
        Ok(())
    }

    pub async fn config_template(&self, _format: FileFormat) -> Result<(), AnyError> {
        println!("{}", "# not implemented yet");
        Ok(())
    }

    pub async fn edit_config_file(&self, config_path: &str) -> Result<(), AnyError> {
        let format = FileFormat::from_path(config_path);
        let body = fs::read_to_string(&config_path)?;
        let mut config = RoozCfg::deserialize_config(&body, format)?.unwrap();
        config.decrypt(self.read_age_identity().await?).await?;
        let decrypted_string = config.to_string(format)?;
        let (encrypted_config, edited_string) = self
            .edit_config_core(decrypted_string.clone(), format)
            .await?;

        if edited_string != decrypted_string {
            fs::write(config_path, encrypted_config.to_string(format)?)?;
        }
        Ok(())
    }
}
