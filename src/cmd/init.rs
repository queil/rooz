use bollard::Docker;

use crate::{image, ssh, types::VolumeResult, volume};

pub async fn init(
    docker: &Docker,
    image: &str,
    uid: &str,
    force: bool,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let image_id = image::ensure_image(&docker, &image, false).await?;

    match volume::ensure_volume(
        &docker,
        ssh::ROOZ_SSH_KEY_VOLUME_NAME.into(),
        "ssh-key",
        Some("ssh-key".into()),
        force,
    )
    .await?
    {
        VolumeResult::Created { .. } => ssh::init_ssh_key(&docker, &image_id, &uid).await?,
        VolumeResult::AlreadyExists => {
            println!("Rooz has been already initialized. Use --force to reinitialize.")
        }
    }

    Ok(())
}
