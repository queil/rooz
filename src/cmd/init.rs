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
    pub async fn init(&self, spec: &InitParams) -> Result<(), AnyError> {
        self.image.ensure(constants::DEFAULT_IMAGE, false).await?;
        match self
            .volume
            .ensure_volume(
                ssh::VOLUME_NAME.into(),
                &RoozVolumeRole::SshKey,
                Some("ssh-key".into()),
                spec.force,
            )
            .await?
        {
            VolumeResult::Created { .. } => {
                let hostname = self.client.info().await?.name.unwrap_or("unknown".into());
                let init_ssh = format!(
                    r#"mkdir -p /tmp/.ssh
                       KEYFILE=/tmp/.ssh/id_ed25519
                       ls "$KEYFILE.pub" > /dev/null 2>&1 || ssh-keygen -t ed25519 -N '' -f $KEYFILE -C rooz@{}
                       cat "$KEYFILE.pub"
                       chmod 400 $KEYFILE && chown -R {} /tmp/.ssh
                    "#,
                    &hostname,
                    &spec.uid.value.unwrap_or(constants::DEFAULT_UID),
                );

                self.container
                    .one_shot(
                        "rooz-init-ssh",
                        init_ssh.into(),
                        Some(vec![Mount {
                            typ: Some(VOLUME),
                            read_only: Some(false),
                            source: Some(ssh::VOLUME_NAME.into()),
                            target: Some("/tmp/.ssh".into()),
                            ..Default::default()
                        }]),
                    )
                    .await?;
            }
            VolumeResult::AlreadyExists => {
                println!("Rooz has been already initialized. Use --force to reinitialize.")
            }
        }

        match self
            .volume
            .ensure_volume(
                api::crypt::VOLUME_NAME.into(),
                &RoozVolumeRole::AgeKey,
                Some("age-key".into()),
                spec.force,
            )
            .await?
        {
            VolumeResult::Created { .. } => {
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
                    r#"mkdir -p /tmp/.age && \
                        echo -n '{}' > /tmp/.age/age.key && \
                        echo -n '{}' > /tmp/.age/age.pub && \
                        chmod 400 /tmp/.age/age.key && \
                        chown -R {} /tmp/.age
                        "#,
                    &key.to_string().expose_secret(),
                    pubkey,
                    &spec.uid.value.unwrap_or(constants::DEFAULT_UID)
                );

                self.container
                    .one_shot(
                        "rooz-init-age",
                        entrypoint,
                        Some(vec![Mount {
                            typ: Some(VOLUME),
                            read_only: Some(false),
                            source: Some(api::crypt::VOLUME_NAME.into()),
                            target: Some("/tmp/.age".into()),
                            ..Default::default()
                        }]),
                    )
                    .await?;
                println!("{}", pubkey);
            }
            VolumeResult::AlreadyExists => {
                println!("Rooz has been already initialized. Use --force to reinitialize.")
            }
        }
        Ok(())
    }
}
