use crate::{
    api::ConfigApi,
    cli::{ConfigFormat, ConfigPart},
    config::config::{ConfigType, FileFormat},
    model::types::AnyError,
};

impl<'a> ConfigApi<'a> {
    async fn show_workspace(
        &self,
        workspace_key: &str,
        format: Option<FileFormat>,
    ) -> Result<String, AnyError> {
        if let Some((origin, cfg)) = self.read_workspace(workspace_key).await? {
            if let Some(fmt) = format {
                Ok(cfg.to_string(fmt)?)
            } else {
                Ok(cfg.to_string(FileFormat::from_path(&origin))?)
            }
        } else {
            Ok("".to_string())
        }
    }

    async fn show_origin(&self, workspace_key: &str) -> Result<String, AnyError> {
        self.read(workspace_key, &ConfigType::Origin).await
    }

    async fn show_runtime(&self, workspace_key: &str) -> Result<String, AnyError> {
        self.read(workspace_key, &ConfigType::Runtime).await
    }

    pub async fn show(
        &self,
        workspace_key: &str,
        part: ConfigPart,
        output: Option<ConfigFormat>,
    ) -> Result<(), AnyError> {
        let result = match part {
            ConfigPart::Origin => self.show_origin(workspace_key).await?,
            ConfigPart::Body => {
                self.show_workspace(
                    workspace_key,
                    output.map(|f| match f {
                        ConfigFormat::Toml => FileFormat::Toml,
                        ConfigFormat::Yaml => FileFormat::Yaml,
                    }),
                )
                .await?
            }
            ConfigPart::Runtime => self.show_runtime(workspace_key).await?,
        };
        println!("{}", result);

        Ok(())
    }
}
