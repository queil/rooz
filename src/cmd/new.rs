use bollard::Docker;

use crate::{
    cli::NewParams,
    git, image, ssh,
    types::{RoozCfg, VolumeResult, WorkSpec},
    volume, workspace,
};

pub async fn new(
    docker: &Docker,
    spec: &NewParams,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let ephemeral = false; // ephemeral containers won't be supported at the moment

    let orig_shell = &spec.work.shell;
    let orig_user = &spec.work.user;
    let orig_uid = "1000".to_string();
    let orig_image = &spec.work.image;

    let orig_image_id = image::ensure_image(&docker, &orig_image, spec.work.pull_image).await?;

    let ssh_key_vol_result = volume::ensure_volume(
        &docker,
        ssh::ROOZ_SSH_KEY_VOLUME_NAME.into(),
        "ssh-key",
        Some("ssh-key".into()),
    )
    .await;

    if let VolumeResult::Created { .. } = ssh_key_vol_result {
        ssh::init_ssh_key(&docker, &orig_image_id, &orig_uid).await?;
    };

    let home_dir = format!("/home/{}", &orig_user);
    let work_dir = format!("{}/work", &home_dir);

    let work_spec = WorkSpec {
        image: &orig_image,
        image_id: &orig_image_id,
        shell: &orig_shell,
        uid: &orig_uid,
        user: &orig_user,
        container_working_dir: &work_dir,
        container_name: &spec.name,
        is_ephemeral: ephemeral,
        git_vol_mount: None,
        caches: spec.work.caches.clone(),
        privileged: spec.work.privileged,
        force_recreate: spec.force,
    };

    match git::clone_repo(
        &docker,
        &orig_image,
        &orig_image_id,
        &orig_uid,
        spec.work.git_ssh_url.clone(),
        &spec.name,
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
            let image_id = image::ensure_image(&docker, &img, spec.work.pull_image).await?;
            let clone_dir = git::get_clone_dir(&work_dir, Some(url.clone()));
            let git_vol_mount = git::git_volume(&docker, &clone_dir, &spec.name).await?;
            let sh = shell.or(Some(orig_shell.to_string())).unwrap();
            let caches = spec.work.caches.clone();
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
            if spec.enter {
                workspace::enter(&docker, &container_id, Some(&clone_dir), &sh).await?
            }
        }
        (None, url) => {
            let clone_dir = git::get_clone_dir(&work_dir, url);
            let git_vol_mount = git::git_volume(&docker, &clone_dir, &spec.name).await?;
            let work_spec = WorkSpec {
                container_working_dir: &clone_dir,
                git_vol_mount: Some(git_vol_mount),
                ..work_spec
            };
            let container_id = workspace::create(&docker, &work_spec).await?;
            if spec.enter {
                workspace::enter(&docker, &container_id, Some(&clone_dir), &work_spec.shell).await?
            }
        }

        _ => {
            let container_id = workspace::create(&docker, &work_spec).await?;
            if spec.enter {
                workspace::enter(
                    &docker,
                    &container_id,
                    Some(&work_spec.container_working_dir),
                    &work_spec.shell.as_ref(),
                )
                .await?
            }
        }
    };
    Ok(())
}
