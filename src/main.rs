use std::io;
use std::str::FromStr;

use rooz::{
    api::{Api, ContainerApi, ExecApi, GitApi, ImageApi, InitApi, VolumeApi, WorkspaceApi},
    cli::{
        Cli,
        Commands::{
            Cache, Code, Config, Doctor, Enter, Exec, Internal, List, Logs, Move, New, Remote,
            Remove, Restart, Start, Status, Stop, System, Tmp, Update,
        },
        CompletionParams, ConfigureParams, ListParams, NewParams, RemoveParams, ShowConfigParams,
        StopParams, TmpParams,
    },
    cmd::remote,
    config::remotes,
    model::types::AnyError,
    util::backend::{ContainerBackend, check_version_floor},
};

use bollard::{API_DEFAULT_VERSION, Docker};
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use colored::Colorize;
use rooz::api::{ConfigApi, CryptApi};
use rooz::cli::{
    CacheCommands, CacheParams, CodeParams, EditConfigParams, EnterParams, ExecParams,
    InternalCommands, InternalParams, LogsParams, MoveParams, RestartParams, StartParams,
    StatusParams, TemplateConfigParams, UpdateParams,
};
use rooz::cmd::plan::PlanMode;
use rooz::cmd::update::UpdateMode;
use rooz::config::config::{ConfigPath, ConfigSource, RoozCfg};
use rooz::constants;
use rooz::util::labels::Labels;
use rooz::util::{id, report};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        let (headline, body) = report::split_error(&e.to_string(), report::terminal_width());
        eprintln!("{} {}", "Error:".red().bold(), headline);
        if !body.is_empty() {
            eprintln!("\n{}", body);
        }
        std::process::exit(1);
    }
}

async fn run() -> Result<(), AnyError> {
    let args = Cli::parse();

    let mut logger = env_logger::Builder::from_default_env();
    if args.verbose {
        logger.filter_module("rooz", log::LevelFilter::Debug);
    }
    logger.init();

    log::debug!("Started");

    if let Remote(params) = &args.command {
        remote::run(params).await?;
        std::process::exit(0);
    }

    if let Config(rooz::cli::Config {
        command: rooz::cli::ConfigCommands::Schema(_),
    }) = &args.command
    {
        println!("{}", RoozCfg::json_schema()?);
        std::process::exit(0);
    }

    if let Config(rooz::cli::Config {
        command: rooz::cli::ConfigCommands::Template(TemplateConfigParams { detect }),
    }) = &args.command
    {
        let detected = match detect {
            Some(dir) => rooz::config::detect::detect(std::path::Path::new(dir))?,
            None => vec![],
        };
        print!("{}", rooz::config::detect::template(&detected));
        std::process::exit(0);
    }

    if let List(ListParams {
        all_hosts: true, ..
    }) = &args.command
    {
        remote::list_all_hosts()?;
        std::process::exit(0);
    }

    let host_socket = args
        .host
        .as_deref()
        .map(remotes::connected_socket)
        .transpose()?;

    let connection = match (
        &host_socket,
        std::env::var("DOCKER_HOST").ok().filter(|s| !s.is_empty()),
    ) {
        (Some(socket), _) => {
            Docker::connect_with_unix(&socket.to_string_lossy(), 120, API_DEFAULT_VERSION)
        }
        (None, Some(h)) if h.starts_with("unix://") => {
            Docker::connect_with_unix(h.trim_start_matches("unix://"), 120, API_DEFAULT_VERSION)
        }
        (None, Some(_)) => Docker::connect_with_http_defaults(),
        (None, None) => Docker::connect_with_local_defaults(),
    };

    let target = rooz::util::backend::engine_target(
        args.host.as_deref(),
        host_socket.as_deref(),
        std::env::var("DOCKER_HOST").ok().as_deref(),
    );
    log::debug!("Engine endpoint: {}", target);

    if let Doctor(_) = &args.command {
        let ok = rooz::cmd::doctor::run(connection, &target).await?;
        std::process::exit(if ok { 0 } else { 1 });
    }

    let docker = connection.map_err(|e| rooz::util::backend::engine_error_message(&target, &e))?;

    log::debug!("Client ver: {}", docker.client_version());
    let (version, info) = tokio::join!(docker.version(), docker.info());
    let (version, info) = match (version, info) {
        (Ok(v), Ok(i)) => (v, i),
        (v, i) => {
            let e = v.err().or(i.err()).expect("one of them failed");
            return Err(rooz::util::backend::engine_error_message(&target, &e).into());
        }
    };

    if let Some(ver) = &version.api_version {
        log::debug!("Server API ver: {}", ver);
    }
    if let Some(components) = &version.components {
        for c in components {
            log::debug!("{}: {}", c.name, c.version.replace('\n', ", "));
        }
    }
    let backend = ContainerBackend::resolve(&version, &info).await?;
    log::debug!("Container backend: {:?}", backend);
    check_version_floor(&version, &backend)?;

    let exec_api = ExecApi { client: &docker };
    let image_api = ImageApi {
        client: &docker,
        backend: &backend,
    };
    let container_api = ContainerApi {
        client: &docker,
        exec: &exec_api,
        image: &image_api,
        backend: &backend,
    };

    let volume_api = VolumeApi {
        client: &docker,
        container: &container_api,
    };

    let rooz = Api {
        exec: &exec_api,
        image: &image_api,
        volume: &volume_api,
        container: &container_api,
        client: &docker,
    };

    let crypt_api = CryptApi {};

    let config_api = ConfigApi {
        api: &rooz,
        crypt: &crypt_api,
    };

    let git_api = GitApi {
        api: &rooz,
        config: &config_api,
    };

    let workspace = WorkspaceApi {
        api: &rooz,
        git: &git_api,
        config: &config_api,
    };

    match args {
        Cli {
            command:
                New(NewParams {
                    work,
                    name,
                    config_path,
                    dry_run,
                }),
            ..
        } => {
            let config_source = match config_path {
                Some(path) => Some(ConfigSource::Path {
                    value: ConfigPath::from_str(&path)?,
                }),
                None => None,
            };

            id::validate_workspace_name(&name)?;

            let labels = Labels::workspace_config(&name);

            match workspace.api.volume.get_single(&labels).await? {
                Some(_) => Err(format!(
                    "Workspace already exists. Did you mean: rooz enter {}? Otherwise, use rooz update to modify the workspace.",
                    name.clone()
                )),
                None => Ok(()),
            }?;

            if dry_run {
                let plan = workspace
                    .plan_new(&name, &work, config_source, PlanMode::New)
                    .await?;
                println!("{}", plan);
                return Ok(());
            }

            workspace
                .new(&name, &work, config_source, false, None)
                .await?;
            println!(
                "\nThe workspace is ready. Run 'rooz enter {}' to enter.",
                name
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
                    chown,
                }),
            ..
        } => {
            workspace
                .enter(
                    &name,
                    work_dir.as_deref(),
                    shell.as_deref().map(|v| vec![v]),
                    container.as_deref(),
                    root,
                    chown,
                )
                .await?;
        }

        Cli {
            command:
                Exec(ExecParams {
                    name,
                    root,
                    work_dir,
                    container,
                    cmd,
                }),
            ..
        } => {
            let code = workspace
                .exec(
                    &name,
                    work_dir.as_deref(),
                    container.as_deref(),
                    root,
                    cmd.iter().map(String::as_str).collect(),
                )
                .await?;
            std::process::exit(code.clamp(0, 255) as i32);
        }

        Cli {
            command: List(ListParams { json, .. }),
            ..
        } => rooz.list(json).await?,

        Cli {
            command:
                Move(MoveParams {
                    name,
                    new_name,
                    to,
                    remove_source,
                    dry_run,
                    allow_secrets,
                }),
            host,
            ..
        } => {
            remote::announce_target(host.as_deref());
            match (to, new_name) {
                (Some(target), new_name) => {
                    workspace
                        .move_to_host(
                            &name,
                            new_name.as_deref().unwrap_or(&name),
                            &target,
                            host.as_deref(),
                            remove_source,
                            dry_run,
                            allow_secrets,
                        )
                        .await?
                }
                (None, Some(new_name)) => {
                    workspace
                        .move_workspace(&name, &new_name, dry_run, allow_secrets)
                        .await?
                }
                (None, None) => unreachable!("clap requires a new name without --to"),
            }
        }

        Cli {
            command:
                Internal(InternalParams {
                    command: InternalCommands::Preflight { name },
                }),
            ..
        } => println!("{}", workspace.preflight_destination(&name).await?),

        Cli {
            command:
                Internal(InternalParams {
                    command: InternalCommands::ReceiveVolume { name, labels },
                }),
            ..
        } => {
            let labels = labels
                .iter()
                .map(|l| {
                    l.split_once('=')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .ok_or(format!("bad label '{}': expected key=value", l))
                })
                .collect::<Result<std::collections::HashMap<_, _>, _>>()?;
            workspace.receive_volume(&name, labels).await?
        }

        Cli {
            command: Cache(CacheParams { command }),
            ..
        } => match command {
            CacheCommands::Ls(p) => rooz.cache_ls(p.all_scopes, p.json).await?,
            CacheCommands::Rm(p) => rooz.cache_rm(&p.names).await?,
            CacheCommands::Prune(p) => rooz.cache_prune(p.dry_run).await?,
        },

        Cli {
            command: Status(StatusParams { name, json }),
            ..
        } => rooz.status(&name, json).await?,

        Cli {
            command:
                Logs(LogsParams {
                    name,
                    container,
                    follow,
                    tail,
                }),
            ..
        } => {
            rooz.logs(&name, container.as_deref(), follow, tail.as_deref())
                .await?
        }

        Cli {
            command:
                Remove(RemoveParams {
                    name: Some(name),
                    force,
                    ..
                }),
            ..
        } => workspace.remove(&name, false, force).await?,

        Cli {
            command: Remove(RemoveParams { name: _, force, .. }),
            host,
            ..
        } => {
            remote::announce_target(host.as_deref());
            workspace.remove_all(force).await?
        }

        Cli {
            command: Stop(StopParams {
                name: Some(name), ..
            }),
            ..
        } => {
            workspace.stop(&name).await?;
        }

        Cli {
            command:
                Restart(RestartParams {
                    name,
                    all_containers,
                }),
            ..
        } => {
            workspace.restart(&name, all_containers).await?;
        }

        Cli {
            command: Start(StartParams { name }),
            ..
        } => {
            workspace.start(&name).await?;
        }

        Cli {
            command: Stop(StopParams { name: None, .. }),
            ..
        } => {
            workspace.stop_all().await?;
        }

        Cli {
            command:
                Update(UpdateParams {
                    name,
                    env,
                    tweak,
                    purge,
                    no_pull,
                    dry_run,
                    allow_secrets,
                }),
            ..
        } => {
            workspace
                .update(
                    &name,
                    &env,
                    tweak,
                    match purge {
                        true => UpdateMode::Purge,
                        _ => UpdateMode::Apply,
                    },
                    no_pull,
                    dry_run,
                    allow_secrets,
                )
                .await?;
        }

        Cli {
            command: Code(CodeParams { name }),
            ..
        } => {
            let docker_host = host_socket
                .as_ref()
                .map(|s| format!("unix://{}", s.display()));
            workspace
                .attach_vscode(&name, docker_host.as_deref())
                .await?;
        }

        Cli {
            command: Tmp(TmpParams { work, root, shell }),
            ..
        } => {
            workspace.tmp(&work, root, &shell).await?;
        }

        Cli {
            command:
                Config(rooz::cli::Config {
                    command: rooz::cli::ConfigCommands::Template(_),
                }),
            ..
        } => {
            unreachable!("handled before connecting to the engine")
        }

        Cli {
            command:
                Config(rooz::cli::Config {
                    command: rooz::cli::ConfigCommands::Edit(EditConfigParams { config_path }),
                }),
            ..
        } => workspace.config.edit(&config_path).await?,

        Cli {
            command:
                Config(rooz::cli::Config {
                    command:
                        rooz::cli::ConfigCommands::Show(ShowConfigParams {
                            name,
                            part,
                            resolved,
                        }),
                }),
            ..
        } => {
            workspace.config.show(&name, part, None, resolved).await?;
        }

        Cli {
            command:
                Config(rooz::cli::Config {
                    command: rooz::cli::ConfigCommands::Schema(_),
                }),
            ..
        } => {
            unreachable!("handled before connecting to the engine")
        }

        Cli {
            command: Remote(_) | Doctor(_),
            ..
        } => {
            unreachable!("handled before connecting to the engine")
        }

        Cli {
            command:
                System(rooz::cli::System {
                    command: rooz::cli::SystemCommands::Prune(_),
                }),
            host,
            ..
        } => {
            remote::announce_target(host.as_deref());
            rooz.prune_system().await?;
        }

        Cli {
            command:
                System(rooz::cli::System {
                    command: rooz::cli::SystemCommands::Init(init_params),
                }),
            ..
        } => {
            let init = InitApi {
                api: &rooz,
                client: &docker,
                image: &image_api,
                volume: &volume_api,
                container: &container_api,
            };
            // takes a legacy engine-side identity off the engine before init touches anything
            rooz.get_system_config().await?;
            init.init(
                constants::DEFAULT_IMAGE,
                constants::DEFAULT_UID,
                &init_params,
            )
            .await?;
        }

        Cli {
            command:
                System(rooz::cli::System {
                    command: rooz::cli::SystemCommands::Completion(CompletionParams { shell }),
                }),
            ..
        } => {
            let mut cli = Cli::command()
                .disable_help_flag(true)
                .disable_help_subcommand(true);
            let name = &cli.get_name().to_string();
            generate(shell, &mut cli, name, &mut io::stdout());
        }

        Cli {
            command:
                System(rooz::cli::System {
                    command: rooz::cli::SystemCommands::Configure(ConfigureParams {}),
                }),
            ..
        } => {
            // reading first moves a legacy engine-side identity onto this machine
            rooz.get_system_config().await?;
            let (edited, _) = config_api
                .system_edit_string(rooz.get_system_config_string().await?.clone())
                .await?;
            if edited.age_key.is_some() {
                eprintln!(
                    "NOTE: 'age_key' is not stored on the engine - dropping it. Use \
                     'rooz system init --age-identity <key>' to set the identity on this machine."
                );
            }
            rooz.write_system_config(&edited).await?;
        }
    };
    Ok(())
}
