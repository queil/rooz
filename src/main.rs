mod cli;
mod cmd;
mod container;
mod git;
mod id;
mod image;
mod labels;
mod ssh;
mod types;
mod volume;

use std::process;

use crate::cli::{Cli, Commands, ListParams, WorkParams};
use crate::id::to_safe_id;
use crate::types::{
    RoozCfg, RoozVolume, RoozVolumeRole, RoozVolumeSharing, RunSpec, VolumeResult, WorkSpec,
};
use bollard::Docker;
use bollard::container::{ListContainersOptions};
use clap::Parser;
use cli::{EnterParams, PruneParams, System};

async fn work<'a>(
    docker: &Docker,
    spec: WorkSpec<'a>,
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

    let mut mounts = volume::ensure_mounts(&docker, volumes, spec.is_ephemeral, &home_dir).await?;

    if let Some(m) = spec.git_vol_mount {
        mounts.push(m.clone());
    }

    let run_spec = RunSpec {
        reason: "work",
        image: &spec.image,
        image_id: &spec.image_id,
        user: Some(&spec.uid),
        work_dir: Some(&spec.container_working_dir),
        container_name: &spec.container_name,
        mounts: Some(mounts),
        entrypoint: Some(vec!["cat"]),
        privileged: spec.privileged,
        force_recreate: spec.force_recreate,
    };

    let r = container::create(&docker, run_spec).await?;

    let work_id = r.id();

    Ok(work_id.to_string())
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    env_logger::init();

    log::debug!("Started");

    let args = Cli::parse();
    let docker = Docker::connect_with_local_defaults().expect("Docker API connection established");

    log::debug!("API connected");

    match args {
        Cli {
            command:
                cli::Commands::System(System {
                    command: cli::SystemCommands::Prune(PruneParams {}),
                }),
            ..
        } => {
            cmd::prune::prune_system(&docker).await?;
        },
        Cli {
            command:
        cli::Commands::New(cli::NewParams {
            work:
                WorkParams {
                    git_ssh_url,
                    image,
                    pull_image,
                    shell,
                    user,
                    //work_dir,
                    //prune,
                    privileged,
                    caches,
                },
            name,
            force,
        }),..}  => 

        {
            let ephemeral = false; // ephemeral containers won't be supported at the moment

            let workspace_key = match &git_ssh_url {
                Some(url) => to_safe_id(&url)?,
                None => "rooz-generic".to_string(),
            };

            // if prune {
            //     cmd::prune::prune_workspace(&docker, &workspace_key).await?;
            //     process::exit(0);
            // }

            let orig_shell = shell;
            let orig_user = user;
            let orig_uid = "1000".to_string();
            let orig_image = image;

            let orig_image_id = image::ensure_image(&docker, &orig_image, pull_image).await?;

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
                container_name: &name,
                is_ephemeral: ephemeral,
                git_vol_mount: None,
                caches: caches.clone(),
                privileged,
                force_recreate: force,
            };

            match git::clone_repo(
                &docker,
                &orig_image,
                &orig_image_id,
                &orig_uid,
                git_ssh_url.clone(),
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
                    let image_id = image::ensure_image(&docker, &img, pull_image).await?;
                    let clone_dir = git::get_clone_dir(&work_dir, Some(url.clone()));
                    let git_vol_mount = git::git_volume(&docker, &url, &clone_dir).await?;
                    let sh = shell.or(Some(orig_shell.to_string())).unwrap();
                    let mut all_caches = vec![];
                    if let Some(caches) = caches {
                        all_caches.extend(caches);
                    }
                    if let Some(caches) = repo_caches {
                        all_caches.extend(caches);
                    };

                    all_caches.dedup();

                    work(
                        &docker,
                        WorkSpec {
                            image: &img,
                            image_id: &image_id,
                            shell: &sh,
                            container_working_dir: &clone_dir,
                            git_vol_mount: Some(git_vol_mount),
                            caches: Some(all_caches),
                            ..work_spec
                        },
                    )
                    .await?;
                    // enter(&docker, &container_id, &clone_dir, &sh).await?
                }
                (None, Some(url)) => {
                    let clone_dir = git::get_clone_dir(&work_dir, git_ssh_url.clone());
                    let git_vol_mount = git::git_volume(&docker, &url, &clone_dir).await?;
                    work(
                        &docker,
                        WorkSpec {
                            container_working_dir: &clone_dir,
                            git_vol_mount: Some(git_vol_mount),
                            ..work_spec
                        },
                    )
                    .await?;
                    //enter(&docker, &container_id, &clone_dir, &work_spec.shell).await?
                }

                _ => {
                    let container_id = work(&docker, work_spec).await?;
                    //enter(&docker, &container_id, &work_spec.container_working_dir, &work_spec.shell).await?
                }
            };
        }
        //this is a prototype, work it out
        Cli {
            command:
                cli::Commands::Enter(EnterParams {
                    name,
                    shell,
                    work_dir,
                }),
            ..
        } => enter(&docker, &name, work_dir.as_deref(), &shell).await?,
        Cli {
            command:
            cli::Commands::List(ListParams {}),
            ..
        } => { 
            cmd::list::list(&docker).await?
        }
        // Cli {
        //     git_ssh_url,
        //     image,
        //     pull_image,
        //     shell,
        //     user,
        //     //work_dir,
        //     prune,
        //     privileged,
        //     caches,
        //     command,
        // } => {
        //     // let ephemeral = false; // ephemeral containers won't be supported at the moment

        //     // let workspace_key = match &git_ssh_url {
        //     //     Some(url) => to_safe_id(&url)?,
        //     //     None => "rooz-generic".to_string(),
        //     // };

        //     // if prune {
        //     //     cmd::prune::prune_workspace(&docker, &workspace_key).await?;
        //     // }

        //     // let orig_shell = shell;
        //     // let orig_user = user;
        //     // let orig_uid = "1000".to_string();
        //     // let orig_image = image;

        //     // let orig_image_id = image::ensure_image(&docker, &orig_image, pull_image).await?;

        //     // let ssh_key_vol_result = volume::ensure_volume(
        //     //     &docker,
        //     //     ssh::ROOZ_SSH_KEY_VOLUME_NAME.into(),
        //     //     "ssh-key",
        //     //     Some("ssh-key".into()),
        //     // )
        //     // .await;

        //     // if let VolumeResult::Created { .. } = ssh_key_vol_result {
        //     //     ssh::init_ssh_key(&docker, &orig_image_id, &orig_uid).await?;
        //     // };

        //     // let home_dir = format!("/home/{}", &orig_user);
        //     // let work_dir = format!("{}/work", &home_dir);

        //     // let work_spec = WorkSpec {
        //     //     image: &orig_image,
        //     //     image_id: &orig_image_id,
        //     //     shell: &orig_shell,
        //     //     uid: &orig_uid,
        //     //     user: &orig_user,
        //     //     container_working_dir: &work_dir,
        //     //     container_name: &workspace_key,
        //     //     is_ephemeral: ephemeral,
        //     //     git_vol_mount: None,
        //     //     caches: caches.clone(),
        //     //     privileged,
        //     // };

        //     // match git::clone_repo(
        //     //     &docker,
        //     //     &orig_image,
        //     //     &orig_image_id,
        //     //     &orig_uid,
        //     //     git_ssh_url.clone(),
        //     // )
        //     // .await?
        //     // {
        //     //     (
        //     //         Some(RoozCfg {
        //     //             image: Some(img),
        //     //             shell,
        //     //             caches: repo_caches,
        //     //             ..
        //     //         }),
        //     //         Some(url),
        //     //     ) => {
        //     //         log::debug!("Image config read from .rooz.toml in the cloned repo");
        //     //         let image_id = image::ensure_image(&docker, &img, pull_image).await?;
        //     //         let clone_dir = git::get_clone_dir(&work_dir, Some(url.clone()));
        //     //         let git_vol_mount = git::git_volume(&docker, &url, &clone_dir).await?;
        //     //         let sh = shell.or(Some(orig_shell.to_string())).unwrap();
        //     //         let mut all_caches = vec![];
        //     //         if let Some(caches) = caches {
        //     //             all_caches.extend(caches);
        //     //         }
        //     //         if let Some(caches) = repo_caches {
        //     //             all_caches.extend(caches);
        //     //         };

        //     //         all_caches.dedup();

        //     //         work(
        //     //             &docker,
        //     //             WorkSpec {
        //     //                 image: &img,
        //     //                 image_id: &image_id,
        //     //                 shell: &sh,
        //     //                 container_working_dir: &clone_dir,
        //     //                 git_vol_mount: Some(git_vol_mount),
        //     //                 caches: Some(all_caches),
        //     //                 ..work_spec
        //     //             },
        //     //         )
        //     //         .await?
        //     //     }
        //     //     (None, Some(url)) => {
        //     //         let clone_dir = git::get_clone_dir(&work_dir, git_ssh_url.clone());
        //     //         let git_vol_mount = git::git_volume(&docker, &url, &clone_dir).await?;
        //     //         work(
        //     //             &docker,
        //     //             WorkSpec {
        //     //                 container_working_dir: &clone_dir,
        //     //                 git_vol_mount: Some(git_vol_mount),
        //     //                 ..work_spec
        //     //             },
        //     //         )
        //     //         .await?
        //     //     }

        //     //     _ => work(&docker, work_spec).await?,
        //     // };
        // }
    };
    Ok(())
}
