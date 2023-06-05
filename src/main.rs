mod cli;
mod cmd;
mod constants;
mod container;
mod git;
mod id;
mod image;
mod labels;
mod ssh;
mod types;
mod volume;
mod workspace;

use crate::{
    cli::{
        Cli,
        Commands::{Enter, List, New, Remove, Stop, System, Tmp},
        InitParams, ListParams, NewParams, RemoveParams, StopParams, TmpParams, WorkParams,
    },
    labels::Labels,
    types::{RoozCfg, WorkSpec},
};

use bollard::Docker;
use clap::Parser;
use cli::EnterParams;

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
                New(NewParams {
                    work,
                    persistence,
                    config,
                }),
            ..
        } => {
            let cfg = match config {
                Some(path) => Some(RoozCfg::from_file(&path)?),
                None => None,
            };

            cmd::new::new(&docker, &work, cfg, Some(persistence)).await?;
        }

        Cli {
            command:
                Enter(EnterParams {
                    name,
                    shell,
                    work_dir,
                }),
            ..
        } => workspace::enter(&docker, &name, work_dir.as_deref(), &shell).await?,

        Cli {
            command: List(ListParams {}),
            ..
        } => cmd::list::list(&docker).await?,

        Cli {
            command:
                Remove(RemoveParams {
                    name: Some(name),
                    force,
                    ..
                }),
            ..
        } => {
            let labels = Labels::new(Some(&name), None);
            cmd::remove::remove(&docker, (&labels).into(), force).await?
        }

        Cli {
            command: Remove(RemoveParams {
                name: None, force, ..
            }),
            ..
        } => {
            let labels = Labels::new(None, None);
            cmd::remove::remove(&docker, (&labels).into(), force).await?
        }

        Cli {
            command: Stop(StopParams {
                name: Some(name), ..
            }),
            ..
        } => {
            let labels = Labels::new(Some(&name), None);
            cmd::stop::stop(&docker, (&labels).into()).await?
        }

        Cli {
            command: Stop(StopParams { name: None, .. }),
            ..
        } => {
            let labels = Labels::new(None, None);
            cmd::stop::stop(&docker, (&labels).into()).await?
        }

        Cli {
            command: Tmp(TmpParams { work }),
            ..
        } => {
            let container_id = cmd::new::new(&docker, &work, None, None).await?;
            container::remove(&docker, &container_id, true).await?;
        }

        Cli {
            command:
                System(cli::System {
                    command: cli::SystemCommands::Prune(_),
                }),
            ..
        } => {
            cmd::prune::prune_system(&docker).await?;
        }

        Cli {
            command:
                System(cli::System {
                    command: cli::SystemCommands::Init(InitParams { force }),
                }),
            ..
        } => {
            cmd::init::init(
                &docker,
                constants::DEFAULT_IMAGE,
                constants::DEFAULT_UID,
                force,
            )
            .await?
        }
    };
    Ok(())
}
