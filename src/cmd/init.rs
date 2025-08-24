use std::str::FromStr;

use crate::{
    api::Api,
    cli::InitParams,
    config::config::SystemConfig,
    constants,
    model::{
        types::{AnyError, VolumeResult},
        volume::{RoozVolume, RoozVolumeRole},
    },
    util::ssh,
};
use age::secrecy::ExposeSecret;

impl<'a> Api<'a> {
    pub async fn init(&self, image: &str, uid: &str, spec: &InitParams) -> Result<(), AnyError> {
        let image_id = self.image.ensure(&image, false).await?.id;

        let age_key = match spec.age_identity.clone() {
            None => age::x25519::Identity::generate(),
            Some(identity) => age::x25519::Identity::from_str(&identity)?,
        };
        self.volume
            .ensure_mounts(
                &vec![RoozVolume::system_config_init(
                    "/tmp/sys",
                    SystemConfig {
                        age_key: Some(age_key.to_string().expose_secret().to_string()),
                        gitconfig: Some(
                            r#"
[core]
  sshCommand = ssh -i /tmp/.ssh/id_ed25519 -o UserKnownHostsFile=/tmp/.ssh/known_hosts
"#
                            .trim()
                            .to_string(),
                        ),
                    },
                )?],
                None,
                Some(constants::ROOT_UID),
            )
            .await?;

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
                    &hostname, &uid,
                );

                self.container
                    .one_shot(
                        "init",
                        init_ssh,
                        Some(vec![ssh::mount("/tmp/.ssh")]),
                        None,
                        Some(&image_id),
                    )
                    .await?;
            }
            VolumeResult::AlreadyExists => {
                println!("Rooz has been already initialized. Use --force to reinitialize.")
            }
        }
        Ok(())
    }
}
