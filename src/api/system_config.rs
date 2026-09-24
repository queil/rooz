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
    // volume, where every user of that engine can read - and write - it. Take it out on first
    // contact, but never adopt it as this machine's identity: rooz cannot tell the operator's
    // own upgraded key from one a co-tenant put there, and adopting the latter would encrypt
    // every secret the operator saves afterwards to somebody else's recipient. It is kept
    // aside for the operator to recognise and install themselves.
    async fn evacuate_engine_identity(&self, engine_key: String) -> Result<(), AnyError> {
        let local = identity::read()?;
        let already_mine = local.as_deref() == Some(engine_key.as_str());
        let kept = if already_mine {
            identity::key_path()?
        } else {
            identity::back_up(&engine_key)?
        };

        let mut config = SystemConfig::from_string(&self.get_system_config_string().await?)?;
        config.age_key = None;
        self.write_system_config(&config).await?;

        let volume = RoozVolume::system_config("/tmp/sys").safe_volume_name();
        let message = if already_mine {
            format!(
                "NOTE: removed the age identity from the engine volume '{}' - it already lives \
                 at {:?} on this machine. Every user of that engine could read it there: if the \
                 engine is shared, treat the identity as compromised, re-key with 'rooz system \
                 init --force' and re-encrypt your secrets.",
                volume, kept
            )
        } else {
            format!(
                "NOTE: an age identity was found in the engine volume '{}' and has been taken \
                 out of it, to {:?}. Rooz does not adopt it: anyone with access to that engine \
                 can write a key there, and using theirs would encrypt your secrets to them. If \
                 you recognise it as your own (rooz kept it in that volume before v0.159), \
                 install it with 'rooz system init --force --age-identity \"$(cat {})\"'{}.",
                volume,
                kept,
                kept.display(),
                match &local {
                    Some(_) => " - which replaces the identity already on this machine",
                    None => "",
                }
            )
        };
        eprintln!("{}", message.yellow());
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
