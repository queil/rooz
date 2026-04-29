use crate::{
    api::ConfigApi,
    cli::{ConfigFormat, ConfigPart},
    config::{
        config::{ConfigType, FileFormat, RoozCfg},
        runtime::RuntimeConfig,
    },
    model::types::AnyError,
    util::labels::{self, Labels, WORKSPACE_CONFIG_ROLE},
};

const EXTENDS_SEPARATOR: &str = "---";

impl<'a> ConfigApi<'a> {
    pub async fn show(
        &self,
        workspace_key: &str,
        part: ConfigPart,
        output: Option<ConfigFormat>,
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

        let content: Option<String> = match part {
            ConfigPart::Origin => Some(
                volume
                    .labels
                    .get(labels::CONFIG_ORIGIN)
                    .unwrap()
                    .to_string(),
            ),
            ConfigPart::Body => {
                let new_format = output.map(|c| match c {
                    ConfigFormat::Yaml => FileFormat::Yaml,
                });

                let body = self.read(workspace_key, &ConfigType::Body).await?;

                let body = if let Some(format) = new_format {
                    let origin_path = volume.labels.get(labels::CONFIG_ORIGIN).unwrap();
                    let original_format = FileFormat::from_path(&origin_path);
                    let cfg = RoozCfg::from_string(&body, original_format)?;
                    cfg.to_string(format)?
                } else {
                    body
                };

                let bases_body = self.read(workspace_key, &ConfigType::Bases).await?;
                if bases_body.is_empty() {
                    Some(body)
                } else {
                    Some(format!("{}\n{}\n{}", body, EXTENDS_SEPARATOR, bases_body))
                }
            }
            ConfigPart::Runtime => {
                let runtime_config = self.read(workspace_key, &ConfigType::Runtime).await?;
                match output {
                    Some(ConfigFormat::Yaml) => {
                        let cfg = RuntimeConfig::from_string(runtime_config.to_string())?;
                        Some(serde_yaml::to_string(&cfg)?)
                    }
                    _ => Some(runtime_config.to_string()),
                }
            }
        };

        println!("{}", content.unwrap_or("N/A".to_string()));

        Ok(())
    }
}
