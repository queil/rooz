use std::str::FromStr;

use crate::{
    api::{self, Api},
    cli::InitParams,
    constants,
    model::{
        types::{AnyError, VolumeResult},
        volume::RoozVolumeRole,
    },
    util::ssh,
};
use age::secrecy::ExposeSecret;
use bollard::models::MountTypeEnum::VOLUME;
use bollard::service::Mount;

impl<'a> Api<'a> {
    async fn init_ssh(&self, spec: &InitParams) -> Result<(), AnyError> {
        let ssh_mount = Some(vec![Mount {
            typ: Some(VOLUME),
            read_only: Some(false),
            source: Some(ssh::VOLUME_NAME.into()),
            target: Some("/tmp/.ssh".into()),
            ..Default::default()
        }]);

        let vol_result = self
            .volume
            .ensure_volume(
                ssh::VOLUME_NAME.into(),
                &RoozVolumeRole::SshKey,
                Some("ssh-key".into()),
                false,
            )
            .await?;

        if spec.force {
            log::debug!("Purging volume: {} ", ssh::VOLUME_NAME);
            self.container
                .one_shot(
                    "purge-ssh-key",
                    (r#"find /tmp/.ssh -type f -exec rm {} \; && echo 'OK'"#).into(),
                    ssh_mount.clone(),
                )
                .await?;
        } else if let VolumeResult::AlreadyExists = vol_result {
            eprintln!("Rooz has been already initialized. Use --force to reinitialize.");
            return Ok(());
        }

        let hostname = self.client.info().await?.name.unwrap_or("unknown".into());
        let init_ssh = format!(
            r#"set -e
               echo "Initializing SSH key"
               mkdir -p /tmp/.ssh
               KEYFILE=/tmp/.ssh/id_ed25519
               ls "$KEYFILE.pub" > /dev/null 2>&1 || ssh-keygen -t ed25519 -N '' -f $KEYFILE -C rooz@{}
               chmod 400 $KEYFILE && chown -R {} /tmp/.ssh               
               echo -n "SSH PUBLIC KEY: "
               cat "$KEYFILE.pub"
            "#,
            &hostname,
            &spec.uid.value.unwrap_or(constants::DEFAULT_UID),
        );

        self.container
            .one_shot("rooz-init-ssh", init_ssh.into(), ssh_mount)
            .await?;
        Ok(())
    }

    async fn init_age(&self, spec: &InitParams) -> Result<(), AnyError> {
        let age_mount = Some(vec![Mount {
            typ: Some(VOLUME),
            read_only: Some(false),
            source: Some(api::crypt::VOLUME_NAME.into()),
            target: Some("/tmp/.age".into()),
            ..Default::default()
        }]);

        let vol_result = self
            .volume
            .ensure_volume(
                api::crypt::VOLUME_NAME.into(),
                &RoozVolumeRole::AgeKey,
                Some("age-key".into()),
                false,
            )
            .await?;

        if spec.force {
            log::debug!("Purging volume: {} ", api::crypt::VOLUME_NAME);
            self.container
                .one_shot(
                    "purge-age-key",
                    (r#"find /tmp/.age -type f -exec rm {} \; && echo 'OK'"#).into(),
                    age_mount.clone(),
                )
                .await?;
        } else if let VolumeResult::AlreadyExists = vol_result {
            eprintln!("Rooz has been already initialized. Use --force to reinitialize.");
            return Ok(());
        }

        let (key, pubkey) = match spec.age_identity.clone() {
            None => {
                let key = age::x25519::Identity::generate();
                let pubkey = key.to_public();
                (key, pubkey)
            }
            Some(identity) => {
                let key = age::x25519::Identity::from_str(&identity)?;
                let pubkey = key.to_public();
                (key, pubkey)
            }
        };

        let entrypoint = format!(
            r#"set -e
               echo "Initializing AGE key"
               mkdir -p /tmp/.age
               echo -n '{}' > /tmp/.age/age.key
               echo -n '{}' > /tmp/.age/age.pub
               chmod 400 /tmp/.age/age.key
               chown -R {} /tmp/.age
               echo -n "AGE PUBLIC KEY: "
               cat /tmp/.age/age.pub
                "#,
            &key.to_string().expose_secret(),
            pubkey,
            &spec.uid.value.unwrap_or(constants::DEFAULT_UID)
        );

        self.container
            .one_shot("rooz-init-age", entrypoint, age_mount)
            .await?;
        Ok(())
    }

    pub async fn init(&self, spec: &InitParams) -> Result<(), AnyError> {
        self.image.ensure(constants::DEFAULT_IMAGE, false).await?;
        if spec.force {
            self.container.stop_all().await?;
        }
        self.init_ssh(spec).await?;
        self.init_age(spec).await?;
        Ok(())
    }
}
