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

use std::io;

use crate::{
    cli::{
        Cli,
        Commands::{Enter, List, New, Remove, Stop, System, Tmp},
        CompletionParams, InitParams, ListParams, NewParams, RemoveParams, StopParams, TmpParams,
    },
    types::RoozCfg,
};

use bollard::Docker;
use clap::{CommandFactory, Parser};
use clap_complete::generate;
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
                    container,
                }),
            ..
        } => {
            workspace::enter(
                &docker,
                &name,
                work_dir.as_deref(),
                None,
                &shell,
                container.as_deref(),
                None,
                constants::DEFAULT_UID,
                false,
            )
            .await?
        }

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
        } => workspace::remove(&docker, &name, force).await?,

        Cli {
            command: Remove(RemoveParams {
                name: None, force, ..
            }),
            ..
        } => workspace::remove_all(&docker, force).await?,

        Cli {
            command: Stop(StopParams {
                name: Some(name), ..
            }),
            ..
        } => {
            workspace::stop(&docker, &name).await?;
        }

        Cli {
            command: Stop(StopParams { name: None, .. }),
            ..
        } => {
            workspace::stop_all(&docker).await?;
        }

        Cli {
            command: Tmp(TmpParams { work }),
            ..
        } => {
            cmd::new::new(&docker, &work, None, None).await?;
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

        Cli {
            command:
                System(cli::System {
                    command: cli::SystemCommands::Completion(CompletionParams { shell }),
                }),
        } => {
            let mut cli = Cli::command()
                .disable_help_flag(true)
                .disable_help_subcommand(true);
            let name = &cli.get_name().to_string();
            generate(shell, &mut cli, name, &mut io::stdout());
        }
    };
    Ok(())
}
