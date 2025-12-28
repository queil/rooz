use std::str::FromStr;

use crate::{
    api::InitApi,
    cli::InitParams,
    config::config::SystemConfig,
    constants,
    model::{
        types::{AnyError, VolumeResult},
        volume::{VolumeBackedPath, RoozVolumeRole},
    },
    util::{labels::Labels, ssh},
};
use age::secrecy::ExposeSecret;

impl<'a> InitApi<'a> {
    async fn init_ssh(&self, image_id: &str, uid: &str) -> Result<(), AnyError> {
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
            .await
    }

    pub async fn init(&self, image: &str, uid: &str, spec: &InitParams) -> Result<(), AnyError> {
        let image_id = self.image.ensure(&image, false).await?.id;

        let age_key = match spec.age_identity.clone() {
            None => age::x25519::Identity::generate(),
            Some(identity) => age::x25519::Identity::from_str(&identity)?,
        };
        if spec.force {
            self.volume
                .ensure_mounts(
                    &vec![VolumeBackedPath::system_config_init(
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
        }
        match self
            .volume
            .ensure_volume(
                ssh::VOLUME_NAME.into(),
                false, // can't really recreate the volume if it is used by workspaces without dropping the workspaces
                Some(Labels::from(&[Labels::role(
                    RoozVolumeRole::SshKey.as_str(),
                )])),
            )
            .await?
        {
            VolumeResult::Created { .. } => self.init_ssh(&image_id, uid).await?,
            VolumeResult::AlreadyExists if spec.force => self.init_ssh(&image_id, uid).await?,
            VolumeResult::AlreadyExists => {
                println!("Rooz has been already initialized. Use --force to reinitialize.")
            }
        }
        Ok(())
    }
}
