use crate::{container, types::RunSpec};
use bollard::models::MountTypeEnum::VOLUME;
use bollard::{service::Mount, Docker};

pub const ROOZ_SSH_KEY_VOLUME_NAME: &'static str = "rooz-ssh-key-vol";

pub async fn init_ssh_key(
    docker: &Docker,
    image: &str,
    uid: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let init_ssh = format!(
        r#"echo "Rooz init"
echo "Running in: $(pwd)"
mkdir -p /tmp/.ssh
ssh-keyscan -t ed25519 github.com 140.82.121.4 140.82.121.3 ::ffff:140.82.121.4 ::ffff:140.82.121.3 >> /tmp/.ssh/known_hosts
KEYFILE=/tmp/.ssh/id_ed25519
ls "$KEYFILE.pub" || ssh-keygen -t ed25519 -N '' -f $KEYFILE -C rooz-access-key
cat "$KEYFILE.pub"
chown -R {} /tmp/.ssh
"#,
        &uid
    );

    let init_entrypoint = container::inject(&init_ssh, "entrypoint.sh");

    let run_spec = RunSpec {
        reason: "init-ssh",
        image,
        image_id: "ignore",
        user: Some("root"),
        work_dir: None,
        container_name: "rooz-init-ssh",
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
    };

    let result = container::create(&docker, run_spec).await?;
    container::start(&docker, result.id()).await?;
    container::container_logs_to_stdout(docker, result.id()).await?;
    container::force_remove(docker, result.id()).await?;

    Ok(())
}

pub fn mount(target: &str) -> Mount {
    Mount {
        typ: Some(VOLUME),
        source: Some(ROOZ_SSH_KEY_VOLUME_NAME.into()),
        target: Some(target.into()),
        read_only: Some(true),
        ..Default::default()
    }
}
