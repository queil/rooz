use crate::{
    api::ConfigApi,
    cli::{ConfigFormat, ConfigPart},
    config::{
        config::{FileFormat, RoozCfg},
        runtime::RuntimeConfig,
    },
    model::types::AnyError,
    util::labels::{self, Labels, WORK_ROLE},
};

impl<'a> ConfigApi<'a> {
    pub async fn show(
        &self,
        workspace_key: &str,
        part: ConfigPart,
        output: Option<ConfigFormat>,
    ) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::workspace(workspace_key), Labels::role(WORK_ROLE)]);

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
}
