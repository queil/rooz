mod api;
mod cli;
mod cmd;
mod config;
mod constants;
mod model;
mod util;

use std::io;

use crate::{
    api::{Api, ContainerApi, ExecApi, GitApi, ImageApi, VolumeApi, WorkspaceApi},
    cli::{
        Cli,
        Commands::{
            Code, Config, Enter, List, New, Remote, Remove, Restart, Start, Stop, System, Tmp,
            Update,
        },
        CompletionParams, ConfigureParams, ListParams, NewParams, RemoveParams, ShowConfigParams,
        StopParams, TmpParams,
    },
    cmd::remote,
    config::config::SystemConfig,
    model::{types::AnyError, volume::RoozVolume},
    util::backend::ContainerBackend,
};

use api::{ConfigApi, CryptApi};
use bollard::{secret::SystemVersion, Docker};
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::{
    CodeParams, EditConfigParams, EnterParams, RestartParams, StartParams, TemplateConfigParams,
    UpdateParams,
};
use cmd::update::UpdateMode;
use config::config::{ConfigPath, ConfigSource, FileFormat};
use util::labels::{self, Labels};

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

    let version: &SystemVersion = &docker.version().await?;
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
    let image_api = ImageApi { client: &docker };
    let container_api = ContainerApi {
        client: &docker,
        image: &image_api,
        backend: &backend,
    };

    let sys_config_result = container_api
        .one_shot_output(
            "read-sys-config",
            "ls /tmp/sys/rooz.config > /dev/null 2>&1 && cat /tmp/sys/rooz.config || echo ''"
                .into(),
            Some(vec![
                RoozVolume::system_config_read("/tmp/sys").to_mount(None)
            ]),
            None,
        )
        .await?;

    let system_config = SystemConfig::from_string(&sys_config_result.data)?;

    let volume_api = VolumeApi {
        client: &docker,
        container: &container_api,
    };

    let rooz = Api {
        exec: &exec_api,
        image: &image_api,
        volume: &volume_api,
        container: &container_api,
        system_config: &system_config,
        client: &docker,
    };

    let crypt_api = CryptApi { };

    let git_api = GitApi { api: &rooz };

    let config_api = ConfigApi {
        api: &rooz,
        crypt: &crypt_api,
    };

    let workspace = WorkspaceApi {
        api: &rooz,
        git: &git_api,
        config: &config_api,
        crypt: &crypt_api,
    };

    match args {
        Cli {
            command:
                New(NewParams {
                    work,
                    name,
                    config_path,
                }),
            ..
        } => {
            let config_source = match config_path {
                Some(path) => Some(ConfigSource::Path {
                    value: ConfigPath::from_str(&path)?,
                }),
                None => None,
            };

            let labels = Labels {
                workspace: Labels::workspace(&name),
                role: Labels::role(labels::ROLE_WORK),
                ..Default::default()
            };

            match workspace.api.container.get_single(&labels).await? {
                    Some(_) => Err(format!("Workspace already exists. Did you mean: rooz enter {}? Otherwise, use rooz update to modify the workspace.", name.clone())),
                    None => Ok(()),
                }?;

            workspace
                .new(&name, &work, config_source, false,)
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
                }),
            ..
        } => {
            workspace
                .enter(
                    &name,
                    work_dir.as_deref(),
                    shell.as_deref().map(|v| vec![v.as_ref()]),
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
        } => rooz.list().await?,

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
                )
                .await?;
        }

        Cli {
            command: Code(CodeParams { name }),
            ..
        } => {
            workspace.attach_vscode(&name).await?;
        }

        Cli {
            command: Tmp(TmpParams { work, root, shell }),
            ..
        } => {
            workspace.tmp(&work, root, &shell).await?;
        }

        Cli {
            command:
                Config(cli::Config {
                    command: cli::ConfigCommands::Template(TemplateConfigParams { format }),
                }),
            ..
        } => {
            workspace
                .config
                .template(match format {
                    cli::ConfigFormat::Toml => FileFormat::Toml,
                    cli::ConfigFormat::Yaml => FileFormat::Yaml,
                })
                .await?;
        }

        Cli {
            command:
                Config(cli::Config {
                    command: cli::ConfigCommands::Edit(EditConfigParams { config_path }),
                }),
            ..
        } => workspace.config.edit(&config_path).await?,

        Cli {
            command:
                Config(cli::Config {
                    command: cli::ConfigCommands::Show(ShowConfigParams { name, part, output }),
                }),
            ..
        } => {
            workspace.config.show(&name, part, output).await?;
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

        Cli {
            command:
                System(cli::System {
                    command: cli::SystemCommands::Configure(ConfigureParams {}),
                }),
        } => {
            let (_, config_string) = config_api
                .system_edit_string(sys_config_result.data.clone())
                .await?;
            volume_api
                .ensure_mounts(
                    &vec![RoozVolume::system_config("/tmp/sys", config_string)],
                    None,
                    Some(constants::ROOT_UID),
                )
                .await?;
        }
    };
    Ok(())
}
