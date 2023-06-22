mod backend;
mod cli;
mod cmd;
mod constants;
mod container;
mod exec;
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
    backend::{Api, ContainerBackend, ExecApi, ImageApi, Client},
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
    log::debug!("Client ver: {}", &docker.client_version());

    let version = &docker.version().await?;
    let info = docker.info().await?;
    let backend = ContainerBackend::resolve(&version, &info).await?;
    log::debug!("Container backend: {:?}", &backend);

    if let Some(ver) = &version.api_version {
        log::debug!("Server API ver: {}", ver);
    }
    if let Some(components) = &version.components {
        for c in components {
            log::debug!("{}: {}", c.name, c.version.replace('\n', ", "));
        }
    }

    let client = Client {
        client: &docker,
        backend: &backend,
    };

    let exec_api = ExecApi { client: &client };
    let image_api = ImageApi { client: &client };
    let rooz = Api {
        exec: &exec_api,
        image: &image_api,
        client: &client
    };

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
            rooz.new(&work, cfg, Some(persistence)).await?;
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
            rooz.enter(
                &name,
                work_dir.as_deref(),
                None,
                &shell,
                container.as_deref(),
                vec![],
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
        } => rooz.remove_workspace(&name, force).await?,

        Cli {
            command: Remove(RemoveParams {
                name: None, force, ..
            }),
            ..
        } => rooz.remove_all_workspaces(force).await?,

        Cli {
            command: Stop(StopParams {
                name: Some(name), ..
            }),
            ..
        } => {
            rooz.stop_workspace(&name).await?;
        }

        Cli {
            command: Stop(StopParams { name: None, .. }),
            ..
        } => {
            rooz.stop_all().await?;
        }

        Cli {
            command: Tmp(TmpParams { work }),
            ..
        } => {
            rooz.new(&work, None, None).await?;
        }

        Cli {
            command:
                System(cli::System {
                    command: cli::SystemCommands::Prune(_),
                }),
            ..
        } => {
            rooz.prune_system().await?;
        }

        Cli {
            command:
                System(cli::System {
                    command: cli::SystemCommands::Init(InitParams { force }),
                }),
            ..
        } => {
            rooz.init(constants::DEFAULT_IMAGE, constants::DEFAULT_UID, force)
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
