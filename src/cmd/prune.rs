use std::collections::HashMap;

use bollard::{
    container::ListContainersOptions,
    service::{ContainerSummary, Volume},
    volume::{ListVolumesOptions, RemoveVolumeOptions},
    Docker,
};

use crate::{container, labels, ssh};

async fn prune(
    docker: &Docker,
    filters: HashMap<&str, Vec<&str>>,
    force: bool
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let ls_container_options = ListContainersOptions {
        all: true,
        filters: filters.clone(),
        ..Default::default()
    };
    for cs in docker.list_containers(Some(ls_container_options)).await? {
        if let ContainerSummary { id: Some(id), .. } = cs {
            log::debug!("Force remove container: {}", &id);
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

            log::debug!("Force remove volume: {}", &v.name);
            docker.remove_volume(&v.name, Some(rm_vol_options)).await?
        }
    }
    log::debug!("Prune success");
    Ok(())
}

pub async fn prune_workspace(
    docker: &Docker,
    workspace_key: &str,
    force: bool
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let group_key_filter = format!("{}={}", labels::GROUP_KEY, &workspace_key);
    let filters = HashMap::from([
        ("label", vec![labels::ROOZ]),
        ("label", vec![&group_key_filter]),
    ]);

    prune(docker, filters, force).await
}

pub async fn prune_system(docker: &Docker) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let filters = HashMap::from([("label", vec![labels::ROOZ])]);

    prune(docker, filters, true).await
}
