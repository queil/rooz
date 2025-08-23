use std::path::Path;

use crate::{
    api::VolumeApi,
    model::{
        types::{AnyError, VolumeResult},
        volume::{RoozVolume, RoozVolumeFile, RoozVolumeRole},
    },
    util::labels::Labels,
};
use base64::{engine::general_purpose, Engine as _};
use bollard::{
    errors::Error::DockerResponseServerError, models::VolumeCreateOptions,
    query_parameters::RemoveVolumeOptions, service::Mount,
};

impl<'a> VolumeApi<'a> {
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
        role: &RoozVolumeRole,
        workspace_key: Option<String>,
        force_recreate: bool,
    ) -> Result<VolumeResult, AnyError> {
        let workspace_key_label = match role {
            RoozVolumeRole::Cache => None,
            _ => workspace_key,
        };

        let labels = Labels::new(workspace_key_label.as_deref(), Some(role.as_str()));

        let create_vol_options = VolumeCreateOptions {
            name: Some(name.into()),
            labels: Some((&labels).into()),
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
        for v in volumes {
            let mount = self.ensure_mount(&v, tilde_replacement).await?;
            if let RoozVolume {
                path,
                files: Some(files),
                ..
            } = v
            {
                self.ensure_file(&v.safe_volume_name(), path, &files, mount.clone(), uid)
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
    ) -> Result<Mount, AnyError> {
        log::debug!("Process volume: {:?}", &volume);
        let mount = volume.to_mount(tilde_replacement);
        self.ensure_volume(
            &mount.source.clone().unwrap(),
            &volume.role,
            volume.key(),
            false,
        )
        .await?;
        Ok(mount)
    }

    async fn ensure_file(
        &self,
        volume_name: &str,
        path: &str,
        files: &Vec<RoozVolumeFile>,
        mount: Mount,
        uid: Option<&str>,
    ) -> Result<(), AnyError> {
        let mut cmd = files
            .iter()
            .map(|f| {
                let p = Path::new(path)
                    .join(&f.file_path)
                    .to_string_lossy()
                    .to_string();
                format!(
                    "echo '{}' | base64 -d > {}",
                    general_purpose::STANDARD.encode(f.data.trim()),
                    p,
                )
            })
            .collect::<Vec<_>>()
            .join(" && ".into());

        if let Some(uid) = uid {
            let chown = format!("chown -R {}:{} {} && ", uid, uid, path);
            cmd = format!("{}{}", chown, cmd)
        }

        self.container
            .one_shot(
                &format!("populate volume: {}", &volume_name),
                cmd,
                Some(vec![mount]),
                None,
                None,
            )
            .await?;

        Ok(())
    }
}
