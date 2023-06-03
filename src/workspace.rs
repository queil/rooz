use bollard::Docker;

use crate::{
    container,
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
    };
    return Ok(container::create(&docker, run_spec).await?.id().to_string());
}

pub async fn enter(
    docker: &Docker,
    container_id: &str,
    working_dir: Option<&str>,
    shell: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {

    container::start(&docker, container_id).await?;
    container::exec_tty(
        "work",
        &docker,
        &container_id,
        true,
        working_dir,
        None,
        Some(vec![shell]),
    )
    .await?;
    Ok(())
}
