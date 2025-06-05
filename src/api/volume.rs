use std::path::Path;

use crate::{
    api::VolumeApi,
    constants,
    model::{
        types::{AnyError, RunSpec, VolumeResult},
        volume::{RoozVolume, RoozVolumeRole},
    },
    util::labels::Labels,
};
use bollard::{errors::Error::DockerResponseServerError, volume::RemoveVolumeOptions};
use bollard::{service::Mount, volume::CreateVolumeOptions};

use super::container;

impl<'a> VolumeApi<'a> {
    async fn create_volume(
        &self,
        options: CreateVolumeOptions<&str>,
    ) -> Result<VolumeResult, AnyError> {
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
        role: &RoozVolumeRole,
        workspace_key: Option<String>,
        force_recreate: bool,
    ) -> Result<VolumeResult, AnyError> {
        let workspace_key_label = match role {
            RoozVolumeRole::Cache => None,
            _ => workspace_key,
        };

        let labels = Labels::new(workspace_key_label.as_deref(), Some(role.as_str()));

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
        tilde_replacement: Option<&str>,
    ) -> Result<Vec<Mount>, AnyError> {
        let mut mounts = vec![];
        for v in volumes {
            log::debug!("Process volume: {:?}", &v);
            let mount = v.to_mount(tilde_replacement);
            self.ensure_volume(&mount.source.clone().unwrap(), &v.role, v.key(), false)
                .await?;

            mounts.push(mount);
        }
        Ok(mounts.clone())
    }

    pub async fn ensure_files(&self, mounts: Vec<RoozVolume>, uid: &str) -> Result<(), AnyError> {
        for m in &mounts {
            match m {
                RoozVolume {
                    files: Some(files),
                    path,
                    ..
                } => {
                    let cmd = files
                        .iter()
                        .map(|f| {
                            format!(
                                "echo '{}' > {}",
                                f.data,
                                Path::new(path)
                                    .join(&f.file_path)
                                    .to_string_lossy()
                                    .to_string()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" && ".into());
                    match self
                        .container
                        .create(RunSpec {
                            image: &constants::DEFAULT_IMAGE,
                            uid,
                            mounts: Some(self.ensure_mounts(&mounts, None).await?),
                            entrypoint: Some(
                                container::inject(&cmd, "entrypoint.sh")
                                    .iter()
                                    .map(String::as_str)
                                    .collect(),
                            ),
                            ..RunSpec::default()
                        })
                        .await?
                    {
                        crate::model::types::ContainerResult::Created { id } => {
                            self.container.start(&id).await?
                        }
                        crate::model::types::ContainerResult::AlreadyExists { .. } => (),
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }
}
