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
mod workspace;

use crate::cli::{
    Cli,
    Commands::{Enter, List, New, Remove, System},
    ListParams, RemoveParams,
};

use bollard::Docker;
use clap::Parser;
use cli::{EnterParams, PruneParams};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    env_logger::init();

    log::debug!("Started");

    let args = Cli::parse();
    let docker = Docker::connect_with_local_defaults().expect("Docker API connection established");

    log::debug!("API connected");

    match args {
        Cli {
            command: New(spec), ..
        } => cmd::new::new(&docker, &spec).await?,
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
            command: Remove(RemoveParams { name, force }),
            ..
        } => {
            cmd::prune::prune_workspace(&docker, &name, force).await?;
        }
        Cli {
            command:
                System(cli::System {
                    command: cli::SystemCommands::Prune(PruneParams {}),
                }),
            ..
        } => {
            cmd::prune::prune_system(&docker).await?;
        }
    };
    Ok(())
}
