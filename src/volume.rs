use crate::{
    ssh,
    types::{RoozVolume, VolumeResult}, labels,
};
use bollard::errors::Error::DockerResponseServerError;
use bollard::models::MountTypeEnum::VOLUME;
use bollard::{service::Mount, volume::CreateVolumeOptions, Docker};
use std::{collections::HashMap, path::Path};

pub async fn ensure_volume(
    docker: &Docker,
    name: &str,
    role: &str,
    group_key: Option<String>,
) -> VolumeResult {
    let group_key = group_key.unwrap_or_default();
    let labels = HashMap::from([
        (labels::ROOZ, "true"),
        (labels::ROLE, role),
        (labels::GROUP_KEY, &group_key),
    ]);

    let create_vol_options = CreateVolumeOptions::<&str> {
        name,
        labels,
        ..Default::default()
    };

    match docker.inspect_volume(&name).await {
        Ok(_) => {
            log::debug!("Reusing an existing {} volume", &name);
            VolumeResult::Reused
        }
        Err(DockerResponseServerError {
            status_code: 404,
            message: _,
        }) => match docker.create_volume(create_vol_options).await {
            Ok(v) => {
                log::debug!("Volume created: {:?}", v.name);
                VolumeResult::Created
            }
            Err(e) => panic!("{}", e),
        },
        Err(e) => panic!("{}", e),
    }
}

pub async fn ensure_mounts(
    docker: &Docker,
    volumes: Vec<RoozVolume>,
    is_ephemeral: bool,
    home_dir: &str,
) -> Result<Vec<Mount>, Box<dyn std::error::Error + 'static>> {
    let mut mounts = vec![ssh::mount(
        Path::new(home_dir)
            .join(".ssh")
            .to_string_lossy()
            .to_string()
            .as_ref(),
    )];

    if is_ephemeral {
        return Ok(mounts.clone());
    }

    for v in volumes {
        log::debug!("Process volume: {:?}", &v);
        let vol_name = v.safe_volume_name()?;

        ensure_volume(&docker, &vol_name, v.role.as_str(), v.group_key()).await;

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
