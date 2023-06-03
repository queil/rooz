use std::collections::HashMap;

use bollard::{
    container::ListContainersOptions,
    service::{ContainerSummary, Volume},
    volume::{ListVolumesOptions, RemoveVolumeOptions},
    Docker,
};

use crate::{container, ssh};

pub async fn remove(
    docker: &Docker,
    filters: HashMap<String, Vec<String>>,
    force: bool,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let ls_container_options = ListContainersOptions {
        all: true,
        filters: filters.clone(),
        ..Default::default()
    };
    let force_display = if force { " (force)" } else { "" };
    for cs in docker.list_containers(Some(ls_container_options)).await? {
        if let ContainerSummary { id: Some(id), .. } = cs {
            log::debug!("Remove container: {}{}", &id, &force_display);
            container::remove(&docker, &id, force).await?
        }
    }

    let ls_vol_options = ListVolumesOptions {
        filters: filters.clone(),
        ..Default::default()
    };

    if let Some(volumes) = docker.list_volumes(Some(ls_vol_options)).await?.volumes {
        let rm_vol_options = RemoveVolumeOptions {
            force,
            ..Default::default()
        };

        for v in volumes {
            match v {
                Volume { ref name, .. } if name == ssh::ROOZ_SSH_KEY_VOLUME_NAME => {
                    continue;
                }
                _ => {}
            };

            log::debug!("Remove volume: {}{}", &v.name, &force_display);
            docker.remove_volume(&v.name, Some(rm_vol_options)).await?
        }
    }
    log::debug!("Prune success");
    Ok(())
}
