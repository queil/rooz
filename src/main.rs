mod cli;
mod cmd;
mod constants;
mod container;
mod filter;
mod git;
mod id;
mod image;
mod labels;
mod ssh;
mod types;
mod volume;
mod workspace;

use crate::cli::{
    Cli,
    Commands::{Enter, List, New, Remove, Stop, System, Tmp},
    InitParams, ListParams, NewParams, RemoveParams, StopParams, TmpParams,
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
                    git_ssh_url,
                    work,
                    persistence,
                }),
            ..
        } => {
            cmd::new::new(&docker, git_ssh_url, &work, Some(persistence)).await?;
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
        } => cmd::remove::remove(&docker, filter::of_workspace(&name), force).await?,

        Cli {
            command: Remove(RemoveParams {
                name: None, force, ..
            }),
            ..
        } => cmd::remove::remove(&docker, filter::all(), force).await?,

        Cli {
            command: Stop(StopParams {
                name: Some(name), ..
            }),
            ..
        } => cmd::stop::stop(&docker, filter::of_workspace(&name)).await?,

        Cli {
            command: Stop(StopParams { name: None, .. }),
            ..
        } => cmd::stop::stop(&docker, filter::all()).await?,

        Cli {
            command: Tmp(TmpParams { git_ssh_url, work }),
            ..
        } => {
            let container_id = cmd::new::new(&docker, git_ssh_url, &work, None).await?;
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
