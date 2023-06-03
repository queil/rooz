use bollard::Docker;

use crate::{
    cli::{WorkParams, WorkspacePersistence},
    constants, git, image,
    types::{RoozCfg, WorkSpec},
    workspace,
};

pub async fn new(
    docker: &Docker,
    git_ssh_url: Option<String>,
    spec: &WorkParams,
    persistence: Option<WorkspacePersistence>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let ephemeral = persistence.is_none();

    let orig_shell = &spec.shell;
    let orig_user = &spec.user;
    let orig_uid = constants::DEFAULT_UID.to_string();
    let orig_image = &spec.image;
    let (workspace_key, force, enter) = match persistence {
        Some(p) => (p.name.to_string(), p.force, p.enter),
        None => (crate::id::random_suffix("rooz-temp-"), false, true),
    };

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
        is_ephemeral: ephemeral,
        git_vol_mount: None,
        caches: spec.caches.clone(),
        privileged: spec.privileged,
        force_recreate: force,
    };

    match git_ssh_url {
        None => {
            let container_id = workspace::create(&docker, &work_spec).await?;
            if enter {
                workspace::enter(
                    &docker,
                    &container_id,
                    Some(&work_spec.container_working_dir),
                    &work_spec.shell.as_ref(),
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
                Some(url),
                &workspace_key,
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
                    let git_vol_mount = git::git_volume(&docker, &clone_dir, &workspace_key).await?;
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
                        workspace::enter(&docker, &container_id, Some(&clone_dir), &sh).await?;
                    }
                    return Ok(container_id);
                }
                (None, url) => {
                    let clone_dir = git::get_clone_dir(&work_dir, url);
                    let git_vol_mount = git::git_volume(&docker, &clone_dir, &workspace_key).await?;
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
