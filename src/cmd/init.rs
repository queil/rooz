use std::str::FromStr;

use crate::{
    api::InitApi,
    cli::InitParams,
    config::config::SystemConfig,
    constants,
    model::{
        types::{AnyError, VolumeResult, VolumeSpec},
        volume::{RoozVolume, RoozVolumeRole, VolumeFile},
    },
    util::{identity, labels::Labels, ssh},
};
use age::secrecy::ExposeSecret;
use colored::Colorize;

impl<'a> InitApi<'a> {
    // The key pair is kept across re-inits because every existing workspace mounts it; only an
    // explicit rotation replaces it. Rotation is the remediation path for a leaked key: the
    // volume cannot simply be deleted while workspaces are using it.
    async fn init_ssh(&self, image_id: &str, uid: &str, rotate: bool) -> Result<(), AnyError> {
        let hostname = self.client.info().await?.name.unwrap_or("unknown".into());
        let init_ssh = format!(
            r#"mkdir -p /tmp/.ssh
                       KEYFILE=/tmp/.ssh/id_ed25519
                       {rotate}
                       ls "$KEYFILE.pub" > /dev/null 2>&1 || ssh-keygen -t ed25519 -N '' -f $KEYFILE -C rooz@{hostname}
                       cat "$KEYFILE.pub"
                       chmod 400 $KEYFILE && chown -R {uid} /tmp/.ssh
                    "#,
            rotate = if rotate {
                r#"rm -f "$KEYFILE" "$KEYFILE.pub""#
            } else {
                ""
            },
            hostname = &hostname,
            uid = &uid,
        );

        self.container
            .one_shot(
                "init",
                init_ssh,
                Some(vec![ssh::mount("/tmp/.ssh", false)]),
                None,
                Some(&image_id),
            )
            .await
    }

    // The identity is written to the operator's machine, never to the engine: every user of
    // a container engine can read its volumes, and this key decrypts every secret the
    // operator has ever encrypted with rooz - including ciphertext committed to repositories.
    fn init_age_identity(&self, spec: &InitParams) -> Result<(), AnyError> {
        let path = identity::key_path()?;
        let existing = identity::read()?;

        if existing.is_some() && !spec.force {
            if spec.age_identity.is_some() {
                return Err(format!(
                    "An age identity already exists at {:?}. Re-run with --force to replace it - \
                     secrets encrypted with the current identity will no longer decrypt.",
                    path
                )
                .into());
            }
            println!("Using the age identity at {:?}", path);
            return Ok(());
        }

        if existing.is_some() {
            eprintln!(
                "{}",
                format!(
                    "WARN: replacing the age identity at {:?}. Secrets encrypted with the old one \
                     will no longer decrypt - back it up first if you still need them.",
                    path
                )
                .yellow()
            );
        }

        let age_key = match spec.age_identity.clone() {
            None => age::x25519::Identity::generate(),
            Some(identity) => age::x25519::Identity::from_str(&identity)?,
        };
        identity::write(age_key.to_string().expose_secret())?;
        println!(
            "Age identity written to {:?}. Back it up: it is the only way to decrypt the secrets \
             in your config files, and it is not stored on the engine.",
            path
        );
        Ok(())
    }

    pub async fn init(&self, image: &str, uid: &str, spec: &InitParams) -> Result<(), AnyError> {
        let image_id = self.image.ensure(&image, false).await?.id;

        self.init_age_identity(spec)?;

        // The system config volume carries the git configuration rooz injects into its own
        // containers - no secret material. Reading it first refuses a foreign volume squatting
        // the name, and an engine without a config gets one now: before, only --force wrote it,
        // so a plain first init left the engine with no git configuration at all.
        let sys_config = RoozVolume::system_config("/tmp/sys");
        let current = SystemConfig::from_string(&self.api.get_system_config_string().await?)?;

        if current.gitconfig.is_none() || spec.force {
            let config = SystemConfig {
                age_key: None,
                gitconfig: Some(
                    r#"
[core]
  sshCommand = ssh -i /tmp/.ssh/id_ed25519 -o UserKnownHostsFile=/tmp/.ssh/known_hosts
"#
                    .trim()
                    .to_string(),
                ),
            };
            self.volume
                .write_files(
                    &sys_config,
                    &[VolumeFile::new_private(
                        constants::SYSTEM_CONFIG_FILE,
                        &SystemConfig::to_string(&config.engine_view())?,
                    )],
                    None,
                )
                .await?;
        }
        // the ssh-key volume is never recreated (even on --force) as it may be
        // used by existing workspaces
        match self
            .volume
            .ensure_volume(&VolumeSpec {
                name: ssh::VOLUME_NAME.into(),
                labels: Some(Labels::from(&[Labels::role(
                    RoozVolumeRole::SshKey.as_str(),
                )])),
            })
            .await?
        {
            VolumeResult::Created { .. } => self.init_ssh(&image_id, uid, false).await?,
            VolumeResult::AlreadyExists if spec.rotate_ssh_key => {
                println!(
                    "Rotating the rooz ssh key pair. Register the new public key below wherever \
                     you used the old one, and de-register the old one - running workspaces pick \
                     up the new key on their next start."
                );
                self.init_ssh(&image_id, uid, true).await?
            }
            VolumeResult::AlreadyExists if spec.force => {
                self.init_ssh(&image_id, uid, false).await?
            }
            VolumeResult::AlreadyExists => {
                println!(
                    "The rooz ssh key is already initialized. Use --rotate-ssh-key to replace it."
                )
            }
        }
        Ok(())
    }
}
