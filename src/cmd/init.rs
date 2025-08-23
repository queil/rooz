use std::str::FromStr;

use crate::{
    api::{container, Api},
    cli::InitParams,
    config::config::SystemConfig,
    constants,
    model::{
        types::{AnyError, RunMode, RunSpec, VolumeResult},
        volume::{RoozVolume, RoozVolumeRole},
    },
    util::{id, labels::Labels, ssh},
};
use age::secrecy::ExposeSecret;
use bollard::models::MountTypeEnum::VOLUME;
use bollard::service::Mount;

impl<'a> Api<'a> {
    async fn execute_init(
        &self,
        container_name: &str,
        entrypoint: &str,
        vol_name: &str,
        vol_mount_path: &str,
        image: &str,
    ) -> Result<(), AnyError> {
        let workspace_key = id::random_suffix("init");
        let entrypoint = container::inject(entrypoint, "entrypoint.sh");
        let labels = Labels::default();
        let run_spec = RunSpec {
            reason: "init",
            image,
            uid: constants::ROOT_UID,
            work_dir: None,
            container_name,
            workspace_key: &workspace_key,
            mounts: Some(vec![Mount {
                typ: Some(VOLUME),
                read_only: Some(false),
                source: Some(vol_name.into()),
                target: Some(vol_mount_path.into()),
                ..Default::default()
            }]),
            entrypoint: Some(entrypoint.iter().map(String::as_str).collect()),
            privileged: false,
            force_recreate: false,
            run_mode: RunMode::Init,
            labels,
            ..Default::default()
        };

        let result = self.container.create(run_spec).await?;
        self.container.start(result.id()).await?;
        self.container.logs_to_stdout(result.id()).await?;
        self.container.remove(result.id(), true).await?;
        Ok(())
    }

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

                self.execute_init(
                    "rooz-init-ssh",
                    &init_ssh,
                    ssh::VOLUME_NAME,
                    "/tmp/.ssh",
                    &image_id,
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
