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

use std::{fs, io, path::Path, thread, time::Duration};

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

use bollard::{Docker, API_DEFAULT_VERSION};
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::EnterParams;
use hyper::{service::service_fn, Request, Response};
use hyper_util::rt::TokioIo;
use openssh::{ForwardType, KnownHosts, Session, SessionBuilder};
use std::{error::Error, io::ErrorKind};
use tokio::net::{TcpListener, TcpStream, UnixListener};

use std::env;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};

use tokio::io::{ AsyncWriteExt as _};


const PHRASE: &str = "It's a Unix system. I know this.\n";

#[tokio::main]
async fn main() -> Result<(), AnyError> {
    env_logger::init();

    log::debug!("Started");

    let args = Cli::parse();

    if let Cli {
        command: Remote(cli::RemoteParams {
            ssh_url,
            local_port,
        }),
    } = &args
    {
        let session = SessionBuilder::default()
            .known_hosts_check(KnownHosts::Accept)
            .connect_timeout(Duration::from_secs(5))
            .connect(&ssh_url)
            .await?;

        println!("SSH:{} connected", &ssh_url);

        
        let local_addr = {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            listener.local_addr()?
        };



        println!("Remote socket candidate: {}", &local_addr.to_string());

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

        let connect_socket = Path::new(&socket_url);

        session
            .request_port_forward(ForwardType::Local, local_addr, connect_socket)
            .await?;

        println!("Remote socket available at: {}", &local_addr.to_string());

        let path = Path::new("/home/queil/.rooz/remote.sock");

        if path.exists() {
            fs::remove_file(path)?;
        }

        let listener = UnixListener::bind(path)?;

        println!("Listening for connections at {}.", path.display());

        loop {
            let (stream, _) = listener.accept().await?;
            let io = TokioIo::new(stream);

            println!("Accepting connection.");



 
            
            tokio::task::spawn(async move {
                let svc_fn = service_fn(|req| async {

                    //req.post().await?

                               
                    // Open a TCP connection to the remote host
                    let stream = TcpStream::connect(&local_addr).await.unwrap();
                    
                    // Use an adapter to access something implementing `tokio::io` traits as if they implement
                    // `hyper::rt` IO traits.
                    let io2 = TokioIo::new(stream);
                    
                    // Create the Hyper client
                    let (mut sender, conn) = hyper::client::conn::http1::handshake(io2).await?;
                    
                    // Spawn a task to poll the connection, driving the HTTP state
                    tokio::task::spawn(async move {
                        if let Err(err) = conn.await {
                            println!("Connection failed: {:?}", err);
                        }
                    });
                    let res = sender.send_request(req).await?;
                    println!("Response: {}", res.status());
                        println!("Headers: {:#?}\n", res.headers());

     
                    Ok::<_, hyper::Error>(res)
                });

                // On linux, serve_connection will return right away with Result::Ok.
                //
                // On OSX, serve_connection will block until the client disconnects,
                // and return Result::Err(hyper::Error) with a source (inner/cause)
                // socket error indicating the client connection is no longer open.
                match hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, svc_fn)
                    .await
                {
                    Ok(()) => {
                        println!("Accepted connection.");
                    }
                    Err(err) => {
                        let source: Option<&std::io::Error> =
                            err.source().and_then(|s| s.downcast_ref());

                        match source {
                            Some(io_err) if io_err.kind() == ErrorKind::NotConnected => {
                                println!("Client disconnected.");
                            }
                            _ => {
                                eprintln!("Failed to accept connection: {err:?}");
                            }
                        }
                    }
                };
            });
        }

        thread::sleep(Duration::from_secs(36000))
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
                    ssh_url,
                    local_port,
                }),
        } => {
            let session = SessionBuilder::default()
                .known_hosts_check(KnownHosts::Accept)
                .connect_timeout(Duration::from_secs(5))
                .connect(&ssh_url)
                .await?;

            println!("SSH:{} connected", &ssh_url);

            let local_addr = {
                let listener = TcpListener::bind(format!("127.0.0.1:{}", local_port)).await?;
                listener.local_addr()?
            };

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
                panic!("Env var DOCKER_HOST is not set on the remote host. Can't get docker.socket path.")
            }

            log::debug!(
                "Read remote socket from env var DOCKER_HOST: {}",
                socket_url
            );

            let connect_socket = Path::new(&socket_url);

            session
                .request_port_forward(ForwardType::Local, local_addr, connect_socket)
                .await?;

            println!("Remote socket available at: {}", &local_addr.to_string());
            thread::sleep(Duration::from_secs(36000))
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
