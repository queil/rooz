use crate::{
    labels::Labels,
    ssh,
    types::{RoozVolume, VolumeResult},
};
use bollard::models::MountTypeEnum::{TMPFS, VOLUME};
use bollard::{errors::Error::DockerResponseServerError, volume::RemoveVolumeOptions};
use bollard::{service::Mount, volume::CreateVolumeOptions, Docker};
use std::path::Path;

async fn create(
    docker: &Docker,
    options: CreateVolumeOptions<&str>,
) -> Result<VolumeResult, Box<dyn std::error::Error + 'static>> {
    match docker.create_volume(options).await {
        Ok(v) => {
            log::debug!("Volume created: {:?}", v.name);
            return Ok(VolumeResult::Created);
        }
        Err(e) => panic!("{}", e),
    }
}

pub async fn ensure_volume(
    docker: &Docker,
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

    match docker.inspect_volume(&name).await {
        Ok(_) if force_recreate => {
            let options = RemoveVolumeOptions { force: true };
            docker.remove_volume(&name, Some(options)).await?;
            return create(docker, create_vol_options).await;
        }
        Ok(_) => {
            log::debug!("Reusing an existing {} volume", &name);
            return Ok(VolumeResult::AlreadyExists);
        }
        Err(DockerResponseServerError {
            status_code: 404,
            message: _,
        }) => return create(docker, create_vol_options).await,
        Err(e) => panic!("{}", e),
    }
}

pub async fn ensure_mounts(
    docker: &Docker,
    volumes: Vec<RoozVolume>,
    home_dir: &str,
    ephemeral: bool,
) -> Result<Vec<Mount>, Box<dyn std::error::Error + 'static>> {
    let mut mounts = vec![ssh::mount(
        Path::new(home_dir).join(".ssh").to_string_lossy().as_ref(),
    )];

    for v in volumes {
        log::debug!("Process volume: {:?}", &v);
        let vol_name = v.safe_volume_name()?;

        if !ephemeral {
            ensure_volume(&docker, &vol_name, v.role.as_str(), v.key(), false).await?;
        }

        let mount = Mount {
            typ: if ephemeral { Some(TMPFS) } else { Some(VOLUME) },
            source: if ephemeral {
                None
            } else {
                Some(vol_name.into())
            },
            target: Some(v.path.replace("~", &home_dir)),
            read_only: Some(false),
            ..Default::default()
        };

        mounts.push(mount);
    }

    Ok(mounts.clone())
}
