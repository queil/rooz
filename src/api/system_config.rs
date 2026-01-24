use std::sync::OnceLock;

use crate::{
    api::Api,
    config::config::SystemConfig,
    model::{types::AnyError, volume::RoozVolume},
};

impl<'a> Api<'a> {
    pub async fn get_system_config_string(&self) -> Result<String, AnyError> {
        let result = self
            .container
            .one_shot_output(
                "read-sys-config",
                "ls /tmp/sys/rooz.config > /dev/null 2>&1 && cat /tmp/sys/rooz.config || echo ''"
                    .into(),
                Some(vec![
                    RoozVolume::system_config_read("/tmp/sys").to_mount(None),
                ]),
                None,
            )
            .await?;

        Ok(result.data)
    }
    pub async fn get_system_config(&self) -> Result<SystemConfig, AnyError> {
        static CACHE: OnceLock<SystemConfig> = OnceLock::new();

        if let Some(config) = CACHE.get() {
            return Ok(config.clone());
        }
        let data = self.get_system_config_string().await?;
        let config = SystemConfig::from_string(&data)?;
        CACHE.set(config.clone()).ok();
        Ok(config)
    }
}
