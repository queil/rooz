use std::path::Path;

use crate::{
    api::VolumeApi,
    model::{
        types::{AnyError, VolumeResult},
        volume::RoozVolume,
    },
    util::labels::Labels,
};
use base64::{engine::general_purpose, Engine as _};
use bollard::{
    errors::Error::DockerResponseServerError,
    models::{Volume, VolumeCreateOptions},
    query_parameters::{ListVolumesOptions, RemoveVolumeOptions},
    service::Mount,
};

impl<'a> VolumeApi<'a> {
    pub async fn get_all(&self, labels: &Labels) -> Result<Vec<Volume>, AnyError> {
        let list_options = ListVolumesOptions {
            filters: Some(labels.clone().into()),
            ..Default::default()
        };

        Ok(self
            .client
            .list_volumes(Some(list_options))
            .await?
            .volumes
            .unwrap_or_default())
    }

    pub async fn get_single(&self, labels: &Labels) -> Result<Option<Volume>, AnyError> {
        match self.get_all(&labels).await?.as_slice() {
            [] => Ok(None),
            [volume] => Ok(Some(volume.clone())),
            _ => panic!("Too many volumes found"),
        }
    }

    async fn create_volume(&self, options: VolumeCreateOptions) -> Result<VolumeResult, AnyError> {
        match &self.client.create_volume(options).await {
            Ok(v) => {
                log::debug!("Volume created: {:?}", v.name);
                return Ok(VolumeResult::Created);
            }
            Err(e) => panic!("{}", e),
        }
    }

    pub async fn remove_volume(&self, name: &str, force: bool) -> Result<(), AnyError> {
        let options = RemoveVolumeOptions { force };
        match &self.client.remove_volume(name, Some(options)).await {
            Ok(_) => {
                let force_display = if force { " (force)" } else { "" };
                log::debug!("Volume removed: {} {}", &name, &force_display);
                return Ok(());
            }
            Err(e) => panic!("{}", e),
        }
    }

    pub async fn ensure_volume(
        &self,
        name: &str,
        force_recreate: bool,
        labels: Option<Labels>,
    ) -> Result<VolumeResult, AnyError> {
        let create_vol_options = VolumeCreateOptions {
            name: Some(name.into()),
            labels: labels.map(|x| x.into()),
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
        tilde_replacement: Option<&str>,
        uid: Option<&str>,
    ) -> Result<Vec<Mount>, AnyError> {
        let mut mounts = vec![];

        let file_volumes = volumes
            .iter()
            .filter(|v| v.file.is_some())
            .collect::<Vec<_>>();

        self.ensure_files(file_volumes, tilde_replacement, uid)
            .await?;

        for v in volumes {
            let mount = if let RoozVolume {
                path,
                file: Some(_),
                ..
            } = v
            {
                self.ensure_mount(
                    &RoozVolume {
                        path: Path::new("/rooz/data")
                            .join(path)
                            .parent()
                            .unwrap()
                            .to_string_lossy()
                            .to_string(),
                        ..v.clone()
                    },
                    tilde_replacement,
                    v.labels.clone(),
                )
                .await?
            } else {
                self.ensure_mount(&v, tilde_replacement, v.labels.clone())
                    .await?
            };

            mounts.push(mount);
        }
        Ok(mounts.clone())
    }

    async fn ensure_mount(
        &self,
        volume: &RoozVolume,
        tilde_replacement: Option<&str>,
        labels: Option<Labels>,
    ) -> Result<Mount, AnyError> {
        log::debug!("Process volume: {:?}", &volume);
        let mount = volume.to_mount(tilde_replacement);
        if let Some(name) = &mount.source {
            self.ensure_volume(&name, false, labels).await?;
        }
        Ok(mount)
    }

    async fn ensure_files(
        &self,
        volumes: Vec<&RoozVolume>,
        tilde_replacement: Option<&str>,
        uid: Option<&str>,
    ) -> Result<(), AnyError> {
        let mut mounts = vec![];
        let mut files_cmd = vec![];
        for v in volumes {
            let init_file_path = Path::new("/rooz/data").join(&v.path.trim_start_matches('/'));
            log::debug!("Init file path: {:?}", init_file_path);
            let x_vol = &RoozVolume {
                path: init_file_path
                    .parent()
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
                ..v.clone()
            };
            mounts.push(
                self.ensure_mount(x_vol, tilde_replacement, v.labels.clone())
                    .await?,
            );
            let file_cmd = format!(
                "echo '{}' | base64 -d > {}",
                general_purpose::STANDARD.encode(x_vol.file.as_ref().unwrap().data.trim()),
                &init_file_path.to_string_lossy().to_string().replace("~", tilde_replacement.unwrap_or("~")),
            );
            files_cmd.push(file_cmd);
        }
        let mut cmd = files_cmd.join(" && ");

        match uid {
            Some(uid) if uid != "0" => {
                let chown = format!(" && chown -R {}:{} /rooz/data", uid, uid,);
                cmd.push_str(chown.as_str());
            }
            _ => (),
        }

        self.container
            .one_shot("populate volumes", cmd, Some(mounts), None, None)
            .await?;

        Ok(())
    }
}
