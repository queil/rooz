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

use std::{fs, io, path::Path, sync::Mutex, time::Duration};

use crate::{
    api::{Api, ContainerApi, ExecApi, GitApi, ImageApi, VolumeApi, WorkspaceApi},
    backend::ContainerBackend,
    cli::{
        Cli,
        Commands::{Describe, Enter, List, New, Remote, Remove, Stop, System, Tmp},
        CompletionParams, DescribeParams, InitParams, ListParams, NewParams, RemoveParams,
        StopParams, TmpParams,
    },
    model::{config::RoozCfg, types::AnyError},
};

use bollard::Docker;
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::EnterParams;
use futures::channel::oneshot::{self, Sender};
use openssh::{ForwardType, KnownHosts,  SessionBuilder};


#[tokio::main]
async fn main() -> Result<(), AnyError> {
    env_logger::init();

    log::debug!("Started");

    let args = Cli::parse();

    if let Cli {
        command: Remote(cli::RemoteParams {
            ssh_url,
            local_socket,
        }),
    } = &args
    {

        let (sender, receiver) = oneshot::channel::<()>();
        
        let tx_mutex = Mutex::<Option<Sender<()>>>::new(Some(sender));

        ctrlc::set_handler(move || {
            if let Some(tx) = tx_mutex.lock().unwrap().take() {
                tx.send(()).unwrap();
            }
        })?;

        let expanded_socket = shellexpand::tilde(&local_socket).into_owned();
        let local_socket_path = Path::new(&expanded_socket);

        if local_socket_path.exists() {
            fs::remove_file(local_socket_path)?;
        }

        let session = SessionBuilder::default()
            .known_hosts_check(KnownHosts::Accept)
            .connect_timeout(Duration::from_secs(5))
            .connect(&ssh_url)
            .await?;

        println!("SSH: connected to {}", &ssh_url);

        let socket_url = String::from_utf8(
            session
                .command("echo")
                .arg("-n")
                .raw_arg("$DOCKER_HOST")
                .output()
                .await?
                .stdout,
        )?;

        if socket_url.is_empty() {
            panic!(
                "Env var DOCKER_HOST is not set on the remote host. Can't get docker.socket path."
            )
        }

        log::debug!(
            "Read remote socket from env var DOCKER_HOST: {}",
            socket_url
        );

        let remote_socket = Path::new(&socket_url);

        session
            .request_port_forward(ForwardType::Local, local_socket_path, remote_socket)
            .await?;

        println!("Forwarding: {} -> {}:{}", local_socket_path.display(), &ssh_url, &remote_socket.display());
        println!("Run 'export DOCKER_HOST=unix://{}' to make the socket useful for local tools", local_socket_path.display());

        futures::executor::block_on(receiver)?;

        if local_socket_path.exists() {
            fs::remove_file(local_socket_path)?;
        }
        std::process::exit(0);
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
                    local_socket: _,
                }),
        } => {
            //TODO: this needs to be handled more elegantly. I.e. Rooz should
            // only connect to Docker API when actually running commands requiring that
            // this command only forwards a local socket to a remote one.
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
