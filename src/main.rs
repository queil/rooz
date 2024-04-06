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

use std::{io, path::Path, time::Duration};

use crate::{
    api::{Api, ContainerApi, ExecApi, GitApi, ImageApi, VolumeApi, WorkspaceApi},
    backend::ContainerBackend,
    cli::{
        Cli,
        Commands::{Describe, Enter, List, New, Remove, Stop, System, Tmp},
        CompletionParams, DescribeParams, InitParams, ListParams, NewParams, RemoveParams,
        StopParams, TmpParams,
    },
    model::{config::RoozCfg, types::AnyError},
};

use bollard::{Docker, API_DEFAULT_VERSION};
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::EnterParams;
use openssh::{ForwardType, KnownHosts, Session, SessionBuilder};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), AnyError> {
    env_logger::init();

    log::debug!("Started");

    let args = Cli::parse();

    // The SSH session is kept here because Docker::connect_with_http doesn't take its ownership.
    let mut _session : Session;

    let connection = if let Cli {
        env_ssh_url: Some(ssh_url),
        command: _,
    } = &args
    {
        _session = SessionBuilder::default()
            .known_hosts_check(KnownHosts::Accept)
            .connect_timeout(Duration::from_secs(5))
            .connect(ssh_url)
            .await?;

        log::debug!("SSH session to {} established", ssh_url);

        let local_addr = {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            listener.local_addr()?
        };

        let socket_url = String::from_utf8(_session.command("echo").arg("-n").raw_arg("$DOCKER_HOST").output().await?.stdout)?;
        
        if socket_url.is_empty() {
            panic!("Env var DOCKER_HOST is not set on the remote host. Can't get docker.socket path.")
        }

        log::debug!("Read remote socket from env var DOCKER_HOST: {}", socket_url);

        let connect_socket = Path::new(&socket_url);

        _session
            .request_port_forward(ForwardType::Local, local_addr, connect_socket)
            .await?;

        Docker::connect_with_http(&local_addr.to_string(), 120, API_DEFAULT_VERSION)
    } else {
        Docker::connect_with_local_defaults()
    };

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
            env_ssh_url: _,
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
