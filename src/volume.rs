use crate::{
    backend::VolumeApi,
    labels::Labels,
    ssh,
    types::{RoozVolume, VolumeResult},
};
use bollard::models::MountTypeEnum::VOLUME;
use bollard::{errors::Error::DockerResponseServerError, volume::RemoveVolumeOptions};
use bollard::{service::Mount, volume::CreateVolumeOptions};
use std::path::Path;

impl<'a> VolumeApi<'a> {
    async fn create_volume(
        &self,
        options: CreateVolumeOptions<&str>,
    ) -> Result<VolumeResult, Box<dyn std::error::Error + 'static>> {
        match &self.client.create_volume(options).await {
            Ok(v) => {
                log::debug!("Volume created: {:?}", v.name);
                return Ok(VolumeResult::Created);
            }
            Err(e) => panic!("{}", e),
        }
    }

    pub async fn remove_volume(
        &self,
        name: &str,
        force: bool,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let options = RemoveVolumeOptions { force };
        match &self.client.remove_volume(name, Some(options)).await {
            Ok(_) => {
                log::debug!("Volume removed: {}", &name);
                return Ok(());
            }
            Err(e) => panic!("{}", e),
        }
    }

    pub async fn ensure_volume(
        &self,
        name: &str,
        role: &str,
        workspace_key: Option<String>,
        force_recreate: bool,
    ) -> Result<VolumeResult, Box<dyn std::error::Error + 'static>> {
        let labels = Labels::new(workspace_key.as_deref(), Some(role));

        let create_vol_options = CreateVolumeOptions::<&str> {
            name,
            labels: (&labels).into(),
            ..Default::default()
        };

        match self.client.inspect_volume(&name).await {
            Ok(_) if force_recreate => {
                let options = RemoveVolumeOptions { force: true };
                self.client.remove_volume(&name, Some(options)).await?;
                return self.create_volume(create_vol_options).await;
            }
            Ok(_) => {
                log::debug!("Reusing an existing {} volume", &name);
                return Ok(VolumeResult::AlreadyExists);
            }
            Err(DockerResponseServerError {
                status_code: 404,
                message: _,
            }) => return self.create_volume(create_vol_options).await,
            Err(e) => panic!("{}", e),
        }
    }

    pub async fn ensure_mounts(
        &self,
        volumes: &Vec<RoozVolume>,
        home_dir: &str,
    ) -> Result<Vec<Mount>, Box<dyn std::error::Error + 'static>> {
        let mut mounts = vec![ssh::mount(
            Path::new(home_dir).join(".ssh").to_string_lossy().as_ref(),
        )];

        for v in volumes {
            log::debug!("Process volume: {:?}", &v);
            let vol_name = v.safe_volume_name();

            self.ensure_volume(&vol_name, v.role.as_str(), v.key(), false)
                .await?;

            let mount = Mount {
                typ: Some(VOLUME),
                source: Some(vol_name.into()),
                target: Some(v.path.replace("~", &home_dir)),
                read_only: Some(false),
                ..Default::default()
            };

            mounts.push(mount);
        }

        Ok(mounts.clone())
    }
}
