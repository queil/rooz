mod age_utils;
mod api;
mod backend;
mod cli;
mod cmd;
mod constants;
mod git;
mod id;
mod labels;
mod model;
mod ssh;

use std::io;

use crate::{
    api::{Api, ContainerApi, ExecApi, GitApi, ImageApi, VolumeApi, WorkspaceApi},
    backend::ContainerBackend,
    cli::{
        Cli,
        Commands::{Describe, Encrypt, Enter, List, New, Remote, Remove, Stop, System, Tmp},
        CompletionParams, DescribeParams, EncryptParams, ListParams, NewParams, RemoveParams,
        StopParams, TmpParams,
    },
    cmd::remote,
    model::{config::RoozCfg, types::AnyError},
};

use bollard::Docker;
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::EnterParams;

#[tokio::main]
async fn main() -> Result<(), AnyError> {
    env_logger::init();

    log::debug!("Started");

    let args = Cli::parse();

    if let Cli {
        command:
            Remote(cli::RemoteParams {
                ssh_url,
                local_docker_host,
            }),
    } = &args
    {
        remote::remote(ssh_url, local_docker_host).await?
    }

    let connection = Docker::connect_with_local_defaults();

    let docker = connection.expect("Docker API connection established");

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

    let exec_api = ExecApi {
        client: &docker,
        backend: &backend,
    };
    let image_api = ImageApi {
        client: &docker,
        backend: &backend,
    };
    let volume_api = VolumeApi {
        client: &docker,
        backend: &backend,
    };
    let container_api = ContainerApi {
        client: &docker,
        backend: &backend,
    };
    let rooz = Api {
        exec: &exec_api,
        image: &image_api,
        volume: &volume_api,
        container: &container_api,
        client: &docker,
        backend: &backend,
    };

    let git_api = GitApi { api: &rooz };

    let workspace = WorkspaceApi {
        api: &rooz,
        git: &git_api,
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
            workspace.new(&work, cfg, Some(persistence.clone())).await?;
            println!(
                "\nThe workspace is ready. Run 'rooz enter {}' to enter.",
                persistence.name
            );
        }

        Cli {
            command:
                Enter(EnterParams {
                    name,
                    shell,
                    root,
                    work_dir,
                    container,
                }),
            ..
        } => {
            workspace
                .enter(
                    &name,
                    work_dir.as_deref(),
                    shell.as_deref(),
                    container.as_deref(),
                    vec![],
                    constants::DEFAULT_UID,
                    root,
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
        } => workspace.remove(&name, force).await?,

        Cli {
            command: Remove(RemoveParams {
                name: None, force, ..
            }),
            ..
        } => workspace.remove_all(force).await?,

        Cli {
            command: Stop(StopParams {
                name: Some(name), ..
            }),
            ..
        } => {
            workspace.stop(&name).await?;
        }

        Cli {
            command: Stop(StopParams { name: None, .. }),
            ..
        } => {
            workspace.stop_all().await?;
        }

        Cli {
            command: Describe(DescribeParams { name, .. }),
            ..
        } => {
            workspace.show_config(&name).await?;
        }

        Cli {
            command: Tmp(TmpParams { work, root, shell }),
            ..
        } => {
            workspace.tmp(&work, root, &shell).await?;
        }

        Cli {
            command:
                Remote(cli::RemoteParams {
                    ssh_url: _,
                    local_docker_host: _,
                }),
        } => {
            //TODO: this needs to be handled more elegantly. I.e. Rooz should
            // only connect to Docker API when actually running commands requiring that
            // this command only forwards a local socket to a remote one.
        }

        Cli {
            command: Encrypt(EncryptParams { config, name }),
        } => {
            let cfg = RoozCfg::from_file(&config)?;
            if let Some(vars) = cfg.vars {
                if vars.contains_key(&name) {
                    let identity = workspace.read_age_identity().await?;
                    let pub_key = identity.to_public();
                    let encrypted = age_utils::encrypt(vars[&name].to_string(), pub_key)?;
                    let mut new_vars = vars.clone();
                    new_vars.insert(name, encrypted);
                    RoozCfg {
                        vars: Some(new_vars),
                        ..cfg
                    }
                    .to_file(&config)?
                }
            } else {
                println!("Var {} not found in {}", &name, &config)
            }
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
                    command: cli::SystemCommands::Init(init_params),
                }),
            ..
        } => {
            rooz.init(
                constants::DEFAULT_IMAGE,
                constants::DEFAULT_UID,
                &init_params,
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
