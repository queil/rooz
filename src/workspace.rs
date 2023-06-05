use std::collections::HashMap;

use bollard::{
    container::ListContainersOptions,
    network::ListNetworksOptions,
    service::{ContainerSummary, Volume},
    volume::{ListVolumesOptions, RemoveVolumeOptions},
    Docker,
};

use crate::{
    container,
    labels::Labels,
    ssh,
    types::{RoozVolume, RoozVolumeRole, RoozVolumeSharing, RunSpec, WorkSpec},
    volume,
};

pub async fn create<'a>(
    docker: &Docker,
    spec: &WorkSpec<'a>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let home_dir = format!("/home/{}", &spec.user);
    let work_dir = format!("{}/work", &home_dir);

    let mut volumes = vec![
        RoozVolume {
            path: home_dir.clone(),
            sharing: RoozVolumeSharing::Exclusive {
                key: spec.container_name.into(),
            },
            role: RoozVolumeRole::Home,
        },
        RoozVolume {
            path: work_dir.clone(),
            sharing: RoozVolumeSharing::Exclusive {
                key: spec.container_name.into(),
            },
            role: RoozVolumeRole::Work,
        },
    ];

    if let Some(caches) = &spec.caches {
        log::debug!("Processing caches");
        let cache_vols = caches
            .iter()
            .map(|p| RoozVolume {
                path: p.to_string(),
                sharing: RoozVolumeSharing::Shared,
                role: RoozVolumeRole::Cache,
            })
            .collect::<Vec<_>>();

        for c in caches {
            log::debug!("Cache: {}", c);
        }

        volumes.extend_from_slice(cache_vols.clone().as_slice());
    } else {
        log::debug!("No caches configured. Skipping");
    }

    let mut mounts = volume::ensure_mounts(&docker, volumes, &home_dir, spec.ephemeral).await?;

    if let Some(m) = &spec.git_vol_mount {
        mounts.push(m.clone());
    }

    let run_spec = RunSpec {
        reason: "work",
        image: &spec.image,
        image_id: &spec.image_id,
        user: Some(&spec.uid),
        work_dir: Some(&spec.container_working_dir),
        container_name: &spec.container_name,
        workspace_key: &spec.workspace_key,
        mounts: Some(mounts),
        entrypoint: Some(vec!["cat"]),
        privileged: spec.privileged,
        force_recreate: spec.force_recreate,
        auto_remove: false,
        labels: spec.labels.clone(),
        network: spec.network,
        ..Default::default()
    };
    return Ok(container::create(&docker, run_spec).await?.id().to_string());
}

async fn remove_core(
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

    let ls_network_options = ListNetworksOptions { filters };
    for n in docker.list_networks(Some(ls_network_options)).await? {
        if let Some(name) = n.name {
            log::debug!("Remove network: {}{}", &name, &force_display);
            docker.remove_network(&name).await?
        }
    }

    log::debug!("Remove success");
    Ok(())
}

pub async fn remove(
    docker: &Docker,
    workspace_key: &str,
    force: bool,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let labels = Labels::new(Some(workspace_key), None);
    remove_core(docker, (&labels).into(), force).await?;
    Ok(())
}

pub async fn remove_all(
    docker: &Docker,
    force: bool,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let labels = Labels::new(None, None);
    remove_core(docker, (&labels).into(), force).await?;
    Ok(())
}

pub async fn start(
    docker: &Docker,
    workspace_key: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let labels = Labels::new(Some(workspace_key), None);
    for c in container::get_all(&docker, labels).await? {
        container::start(&docker, &c.id.unwrap()).await?;
    }
    Ok(())
}

pub async fn stop(
    docker: &Docker,
    workspace_key: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let labels = Labels::new(Some(workspace_key), None);
    for c in container::get_all(&docker, labels).await? {
        container::stop(&docker, &c.id.unwrap()).await?;
    }
    Ok(())
}

pub async fn stop_all(docker: &Docker) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let labels = Labels::new(None, None);
    for c in container::get_all(&docker, labels).await? {
        container::stop(&docker, &c.id.unwrap()).await?;
    }
    Ok(())
}

pub async fn enter(
    docker: &Docker,
    workspace_key: &str,
    working_dir: Option<&str>,
    shell: &str,
    container: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    start(docker, workspace_key).await?;

    container::exec_tty(
        "work",
        &docker,
        &container.unwrap_or(workspace_key),
        true,
        working_dir,
        None,
        Some(vec![shell]),
    )
    .await?;
    Ok(())
}
