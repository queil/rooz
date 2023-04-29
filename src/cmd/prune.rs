use std::collections::HashMap;

use bollard::{
    container::ListContainersOptions,
    service::{ContainerSummary, Volume},
    volume::{ListVolumesOptions, RemoveVolumeOptions},
    Docker,
};

use crate::{container, ssh, labels};

pub async fn prune(
    docker: &Docker,
    group_key: &str,
    prune_all: bool,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let ls_container_options = ListContainersOptions {
        all: true,
        filters: HashMap::from([("label", vec![labels::ROOZ])]),
        ..Default::default()
    };
    for cs in docker.list_containers(Some(ls_container_options)).await? {
        if let ContainerSummary { id: Some(id), .. } = cs {
            log::debug!("Force remove container: {}", &id);
            container::force_remove(&docker, &id).await?
        }
    }

    let group_key_filter = labels::belongs_to(&group_key);
    let mut filters = HashMap::from([("label", vec![labels::ROOZ])]);
    if !prune_all {
        filters.insert("label", vec![&group_key_filter]);
    }
    let ls_vol_options = ListVolumesOptions {
        filters,
        ..Default::default()
    };

    if let Some(volumes) = docker.list_volumes(Some(ls_vol_options)).await?.volumes {
        let rm_vol_options = RemoveVolumeOptions {
            force: true,
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
