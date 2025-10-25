use std::fs::{self};

use crate::{
    api::ConfigApi,
    config::config::{FileFormat, RoozCfg},
    model::types::AnyError,
};

impl<'a> ConfigApi<'a> {
    pub async fn edit(&self, config_path: &str) -> Result<(), AnyError> {
        let format = FileFormat::from_path(config_path);
        let body = fs::read_to_string(&config_path)?;
        let mut config = RoozCfg::deserialize_config(&body, format)?.unwrap();
        let identity = self.api.get_system_config().await?.age_identity()?;
        self.decrypt(&mut config, &identity).await?;
        let decrypted_string = config.to_string(format)?;
        let (encrypted_config, edited_string) = self
            .edit_string(decrypted_string.clone(), format, &identity)
            .await?;

        if edited_string != decrypted_string {
            fs::write(config_path, encrypted_config.to_string(format)?)?;
        }
        Ok(())
    }
}
