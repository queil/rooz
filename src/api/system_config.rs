use std::sync::OnceLock;

use colored::Colorize;

use crate::{
    api::Api,
    config::config::SystemConfig,
    constants,
    model::{
        types::AnyError,
        volume::{RoozVolume, VolumeFile},
    },
    util::identity,
};

impl<'a> Api<'a> {
    pub async fn get_system_config_string(&self) -> Result<String, AnyError> {
        // mounting a missing volume would have the engine create it unlabelled, which the
        // next ownership check would then refuse - so create it through the volume API
        self.volume
            .ensure_volume(&RoozVolume::system_config("/tmp/sys").to_spec())
            .await?;

        let result = self
            .container
            .one_shot_output(
                "read-sys-config",
                format!(
                    "ls /tmp/sys/{f} > /dev/null 2>&1 && cat /tmp/sys/{f} || true",
                    f = constants::SYSTEM_CONFIG_FILE
                ),
                Some(vec![RoozVolume::system_config("/tmp/sys").to_mount(None)]),
                None,
                None,
            )
            .await?;

        Ok(result.data)
    }

    pub async fn write_system_config(&self, config: &SystemConfig) -> Result<(), AnyError> {
        self.volume
            .write_files(
                &RoozVolume::system_config("/tmp/sys"),
                &[VolumeFile::new_private(
                    constants::SYSTEM_CONFIG_FILE,
                    // the identity never goes back to the engine, whatever the caller holds
                    &SystemConfig::to_string(&config.engine_view())?,
                )],
                None,
            )
            .await
    }

    // Versions of rooz before the identity moved off the engine kept it in the system config
    // volume, where every user of that engine can read it. Take it out on first contact.
    async fn evacuate_engine_identity(&self, engine_key: String) -> Result<(), AnyError> {
        let kept = match identity::read()? {
            None => identity::write(&engine_key)?,
            Some(local) if local == engine_key => identity::key_path()?,
            Some(_) => identity::back_up(&engine_key)?,
        };

        let mut config = SystemConfig::from_string(&self.get_system_config_string().await?)?;
        config.age_key = None;
        self.write_system_config(&config).await?;

        eprintln!(
            "{}",
            format!(
                "NOTE: moved the age identity out of the engine volume '{}' - it now lives at \
                 {:?}. Every user of that engine could read it there: if the engine is shared, \
                 treat the identity as compromised, re-key with 'rooz system init --force' and \
                 re-encrypt your secrets.",
                RoozVolume::system_config("/tmp/sys").safe_volume_name(),
                kept
            )
            .yellow()
        );
        Ok(())
    }

    pub async fn get_system_config(&self) -> Result<SystemConfig, AnyError> {
        static CACHE: OnceLock<SystemConfig> = OnceLock::new();

        if let Some(config) = CACHE.get() {
            return Ok(config.clone());
        }
        let data = self.get_system_config_string().await?;
        let mut config = SystemConfig::from_string(&data)?;

        if let Some(engine_key) = config.age_key.take() {
            self.evacuate_engine_identity(engine_key).await?;
        }
        config.age_key = identity::read()?;

        CACHE.set(config.clone()).ok();
        Ok(config)
    }
}
