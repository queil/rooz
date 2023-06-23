use crate::backend::Api;
use crate::labels::Labels;
use crate::{constants, id};
use crate::{container, types::RunSpec};
use bollard::models::MountTypeEnum::VOLUME;
use bollard::service::Mount;

pub const ROOZ_SSH_KEY_VOLUME_NAME: &'static str = "rooz-ssh-key-vol";

pub fn mount(target: &str) -> Mount {
    Mount {
        typ: Some(VOLUME),
        source: Some(ROOZ_SSH_KEY_VOLUME_NAME.into()),
        target: Some(target.into()),
        ..Default::default()
    }
}

impl<'a> Api<'a> {
    pub async fn init_ssh_key(
        &self,
        image: &str,
        uid: &str,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let hostname = hostname::get()?;
        let init_ssh = format!(
            r#"mkdir -p /tmp/.ssh
KEYFILE=/tmp/.ssh/id_ed25519
ls "$KEYFILE.pub" > /dev/null 2>&1 || ssh-keygen -t ed25519 -N '' -f $KEYFILE -C rooz@{}
cat "$KEYFILE.pub"
chmod 400 $KEYFILE && chown -R {} /tmp/.ssh
"#,
            &hostname.to_string_lossy(),
            &uid,
        );

        let init_entrypoint = container::inject(&init_ssh, "entrypoint.sh");
        let labels = Labels::new(None, None);

        let workspace_key = id::random_suffix("init");
        let run_spec = RunSpec {
            reason: "init-ssh",
            image,
            uid: constants::ROOT_UID,
            work_dir: None,
            container_name: "rooz-init-ssh",
            workspace_key: &workspace_key,
            mounts: Some(vec![Mount {
                typ: Some(VOLUME),
                read_only: Some(false),
                source: Some(ROOZ_SSH_KEY_VOLUME_NAME.into()),
                target: Some("/tmp/.ssh".into()),
                ..Default::default()
            }]),
            entrypoint: Some(init_entrypoint.iter().map(String::as_str).collect()),
            privileged: false,
            force_recreate: false,
            auto_remove: true,
            labels: (&labels).into(),
            ..Default::default()
        };

        let result = self.container.create(run_spec).await?;
        self.container.start(result.id()).await?;
        self.container.logs_to_stdout(result.id()).await?;
        Ok(())
    }
}
