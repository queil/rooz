use crate::{backend::Api, ssh, types::VolumeResult};

impl<'a> Api<'a> {
    pub async fn init(
        &self,
        image: &str,
        uid: &str,
        force: bool,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let image_id = self.image.ensure(&image, false).await?;

        match self
            .volume
            .ensure_volume(
                ssh::ROOZ_SSH_KEY_VOLUME_NAME.into(),
                "ssh-key",
                Some("ssh-key".into()),
                force,
            )
            .await?
        {
            VolumeResult::Created { .. } => self.init_ssh_key(&image_id, &uid).await?,
            VolumeResult::AlreadyExists => {
                println!("Rooz has been already initialized. Use --force to reinitialize.")
            }
        }

        Ok(())
    }
}
