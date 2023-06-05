use bollard::{network::CreateNetworkOptions, Docker};

use crate::{
    cli::{WorkParams, WorkspacePersistence},
    constants, container, git, image,
    labels::{self, Labels},
    types::{RoozCfg, RunSpec, WorkSpec},
    workspace,
};

pub async fn new(
    docker: &Docker,
    spec: &WorkParams,
    config: Option<RoozCfg>,
    persistence: Option<WorkspacePersistence>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let ephemeral = persistence.is_none();

    let orig_shell = &spec.shell;
    let orig_user = &spec.user;
    let orig_uid = constants::DEFAULT_UID.to_string();
    let orig_image = &spec.image;
    let (workspace_key, force, enter) = match persistence {
        Some(p) => (p.name.to_string(), p.force, p.enter),
        None => (crate::id::random_suffix("tmp"), false, true),
    };

    let labels = Labels::new(Some(&workspace_key), Some(labels::ROLE_WORK));
    if force {
        workspace::remove(docker, &workspace_key, true).await?;
    }

    let labels_sidecar = Labels::new(Some(&workspace_key), Some(labels::ROLE_SIDECAR));

    let network = if let Some(RoozCfg {
        sidecars: Some(_), ..
    }) = &config
    {
        let network_options = CreateNetworkOptions::<&str> {
            name: &workspace_key,
            check_duplicate: true,
            labels: (&labels).into(),

            ..Default::default()
        };

        docker.create_network(network_options).await?;
        Some(workspace_key.as_ref())
    } else {
        None
    };

    if let Some(RoozCfg {
        sidecars: Some(sidecars),
        ..
    }) = &config
    {
        for (name, s) in sidecars {
            log::debug!("Process sidecar: {}", name);
            image::ensure_image(docker, &s.image, spec.pull_image).await?;
            let container_name = format!("{}-{}", workspace_key, name);
            container::create(
                docker,
                RunSpec {
                    container_name: &container_name,
                    image: &s.image,
                    force_recreate: force,
                    workspace_key: &workspace_key,
                    labels: (&labels_sidecar).into(),
                    env: s.env.clone(),
                    network,
                    network_aliases: Some(vec![name.into()]),
                    ..Default::default()
                },
            )
            .await?;
        }
    }

    let orig_image_id = image::ensure_image(&docker, &orig_image, spec.pull_image).await?;

    let home_dir = format!("/home/{}", &orig_user);
    let work_dir = format!("{}/work", &home_dir);

    let work_spec = WorkSpec {
        image: &orig_image,
        image_id: &orig_image_id,
        shell: &orig_shell,
        uid: &orig_uid,
        user: &orig_user,
        container_working_dir: &work_dir,
        container_name: &workspace_key,
        workspace_key: &workspace_key,
        labels: (&labels).into(),
        ephemeral,
        git_vol_mount: None,
        caches: spec.caches.clone(),
        privileged: spec.privileged,
        force_recreate: force,
        network,
    };

    match &spec.git_ssh_url {
        None => {
            let container_id = workspace::create(&docker, &work_spec).await?;
            if enter {
                workspace::enter(
                    &docker,
                    &container_id,
                    Some(&work_spec.container_working_dir),
                    &work_spec.shell.as_ref(),
                    None,
                )
                .await?;
            }
            return Ok(container_id);
        }
        Some(url) => {
            match git::clone_repo(
                &docker,
                &orig_image,
                &orig_image_id,
                &orig_uid,
                Some(url.into()),
                &workspace_key,
                ephemeral,
            )
            .await?
            {
                (
                    Some(RoozCfg {
                        image: Some(img),
                        shell,
                        caches: repo_caches,
                        ..
                    }),
                    Some(url),
                ) => {
                    log::debug!("Image config read from .rooz.toml in the cloned repo");
                    let image_id = image::ensure_image(&docker, &img, spec.pull_image).await?;
                    let clone_dir = git::get_clone_dir(&work_dir, Some(url.clone()));
                    let git_vol_mount =
                        git::git_volume(&docker, &clone_dir, &workspace_key, ephemeral).await?;
                    let sh = shell.or(Some(orig_shell.to_string())).unwrap();
                    let caches = spec.caches.clone();
                    let mut all_caches = vec![];
                    if let Some(caches) = caches {
                        all_caches.extend(caches);
                    }
                    if let Some(caches) = repo_caches {
                        all_caches.extend(caches);
                    };

                    all_caches.dedup();

                    let work_spec = WorkSpec {
                        image: &img,
                        image_id: &image_id,
                        shell: &sh,
                        container_working_dir: &clone_dir,
                        git_vol_mount: Some(git_vol_mount),
                        caches: Some(all_caches),
                        ..work_spec
                    };

                    let container_id = workspace::create(&docker, &work_spec).await?;
                    if enter {
                        workspace::enter(&docker, &container_id, Some(&clone_dir), &sh, None)
                            .await?;
                    }
                    return Ok(container_id);
                }
                (None, url) => {
                    let clone_dir = git::get_clone_dir(&work_dir, url);
                    let git_vol_mount =
                        git::git_volume(&docker, &clone_dir, &workspace_key, ephemeral).await?;
                    let work_spec = WorkSpec {
                        container_working_dir: &clone_dir,
                        git_vol_mount: Some(git_vol_mount),
                        ..work_spec
                    };
                    let container_id = workspace::create(&docker, &work_spec).await?;
                    if enter {
                        workspace::enter(
                            &docker,
                            &container_id,
                            Some(&clone_dir),
                            &work_spec.shell,
                            None,
                        )
                        .await?;
                    }
                    return Ok(container_id);
                }
                s => {
                    println!("{:?}", s);
                    unreachable!("Unreachable");
                }
            }
        }
    };
}
