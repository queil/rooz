use base64::{engine::general_purpose, Engine as _};
use bollard::container::LogOutput::Console;
use bollard::container::{
    Config, CreateContainerOptions, LogsOptions, RemoveContainerOptions, StartContainerOptions,
};
use bollard::errors::Error::DockerResponseServerError;
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::MountTypeEnum::{BIND, VOLUME};
use bollard::models::{CreateImageInfo, HostConfig, Mount};
use bollard::service::{ContainerConfig, ContainerInspectResponse, ImageInspect};
use bollard::volume::CreateVolumeOptions;
use bollard::Docker;
use clap::{Parser, Subcommand};
use futures::stream::StreamExt;
use regex::Regex;
use std::collections::HashMap;
use std::io::{stdout, Read, Write};
use std::time::Duration;
#[cfg(not(windows))]
use termion::raw::IntoRawMode;
#[cfg(not(windows))]
use termion::{async_stdin, terminal_size};
use tokio::io::AsyncWriteExt;
use tokio::task::spawn;
use tokio::time::sleep;

//TODO: make open git-ssh-url optional and maybe allow specify a container name so it can be used in a "free-style" mode
//TODO: on successfull load clear screen rather than just before cursor - it looks werd if there is a less than a whole
//----- screen of text

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(arg_required_else_help = true)]
    Open {
        #[arg(short, long)]
        git_ssh_url: String,
        #[arg(short, long, default_value = "alpine/git:latest", env = "ROOZ_IMAGE")]
        image: String,
        #[arg(short, long, env = "ROOZ_USER")]
        user: Option<String>,
        #[arg(short, long)]
        work_dir: Option<String>,
        //#[arg(short, long)] // TODO
        //ephemeral: bool,
    },
    Init {
        #[arg(short, long, default_value = "alpine/git:latest", env = "ROOZ_IMAGE")]
        image: String,
        #[arg(short, long, env = "ROOZ_USER")]
        user: Option<String>,
        #[arg(short, long)]
        work_dir: Option<String>,
    },
}

#[derive(Debug, Clone)]
enum ContainerResult {
    Created { id: String },
    Reused { id: String },
}

impl ContainerResult {
    pub fn id(&self) -> &str {
        match self {
            ContainerResult::Created { id } => &id,
            ContainerResult::Reused { id } => &id,
        }
    }
}

async fn exec(
    docker: &Docker,
    container_id: &str,
    interactive: bool, // this is a hack only needed to avoid the garbage bytes written to TTY when opening a new exec
    working_dir: Option<&str>,
    cmd: Option<Vec<&str>>,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    #[cfg(not(windows))]
    {
        let tty_size = terminal_size()?;
        let exec = docker
            .create_exec(
                &container_id,
                CreateExecOptions {
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    attach_stdin: Some(true),
                    tty: Some(true),
                    cmd,
                    working_dir,
                    ..Default::default()
                },
            )
            .await?
            .id;

        if let StartExecResults::Attached {
            mut output,
            mut input,
        } = docker.start_exec(&exec, None).await?
        {
            // pipe stdin into the docker exec stream input
            let handle = spawn(async move {
                if interactive {
                    let mut stdin = async_stdin().bytes();
                    loop {
                        if let Some(Ok(byte)) = stdin.next() {
                            input.write(&[byte]).await.ok();
                        } else {
                            sleep(Duration::from_millis(10)).await;
                        }
                    }
                }
            });

            if interactive {
                match docker
                    .resize_exec(
                        &exec,
                        ResizeExecOptions {
                            height: tty_size.1,
                            width: tty_size.0,
                        },
                    )
                    .await
                {
                    Ok(_) => (),
                    Err(err) => println!("Resize exec: {:?}", err),
                };
            };

            // set stdout in raw mode so we can do tty stuff
            let stdout = stdout();
            let mut stdout = stdout.lock().into_raw_mode()?;
            println!("{}", termion::clear::BeforeCursor);

            // pipe docker exec output into stdout
            while let Some(Ok(output)) = output.next().await {
                stdout.write_all(output.into_bytes().as_ref())?;
                stdout.flush()?;
            }

            handle.abort();
        }
    }
    Ok(())
}

async fn run(
    docker: &Docker,
    image: &str,
    image_id: &str,
    user: Option<&str>,
    work_dir: Option<&str>,
    container_name: &str,
    mounts: Option<Vec<Mount>>,
    log: bool,
    entrypoint: Option<Vec<&str>>,
) -> Result<ContainerResult, Box<dyn std::error::Error + 'static>> {
    println!("Running {}", container_name);

    let container_id = match docker.inspect_container(container_name, None).await {
        Ok(ContainerInspectResponse {
            id: Some(id),
            image: Some(img),
            ..
        }) if img.to_owned() == image_id => ContainerResult::Reused { id },
        s => {
            let remove_options = RemoveContainerOptions {
                force: true,
                ..Default::default()
            };

            if let Ok(ContainerInspectResponse { id: Some(id), .. }) = s {
                docker.remove_container(&id, Some(remove_options)).await?;
            }

            let options = CreateContainerOptions {
                name: container_name,
                platform: None,
            };

            let host_config = HostConfig {
                auto_remove: Some(true),
                mounts,
                ..Default::default()
            };

            let config = Config {
                image: Some(image),
                entrypoint,
                working_dir: work_dir,
                user,
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(true),
                open_stdin: Some(true),
                host_config: Some(host_config),
                ..Default::default()
            };

            ContainerResult::Created {
                id: docker
                    .create_container(Some(options.clone()), config.clone())
                    .await?
                    .id,
            }
        }
    };

    docker
        .start_container(&container_id.id(), None::<StartContainerOptions<String>>)
        .await?;

    if log {
        let log_options = LogsOptions::<String> {
            stdout: true,
            follow: true,
            ..Default::default()
        };

        let mut stream = docker.logs(&container_name, Some(log_options));

        while let Some(l) = stream.next().await {
            match l {
                Ok(Console { message: m }) => stdout().write_all(&m)?,
                Ok(msg) => panic!("{}", msg),
                Err(e) => panic!("{}", e),
            };
        }
    }

    Ok(container_id.clone())
}

fn inject(script: &str, name: &str) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "echo '{}' | base64 -d > /tmp/{} && chmod +x /tmp/{} && /tmp/{}",
            general_purpose::STANDARD.encode(script.trim()),
            name,
            name,
            name
        ),
    ]
}

async fn ensure_volume(docker: &Docker, name: &str, role: &str, unique_id: &str) {
    let static_data_vol_options = CreateVolumeOptions::<&str> {
        name: name,
        labels: HashMap::from([("dev.rooz.role", role), ("dev.rooz.id", unique_id)]),
        ..Default::default()
    };

    match docker.inspect_volume(&name).await {
        Ok(_) => println!("Reusing an existing static-data volume"),
        Err(DockerResponseServerError {
            status_code: 404,
            message: _,
        }) => match docker.create_volume(static_data_vol_options).await {
            Ok(v) => println!("Volume created: {:?}", v.name),
            Err(e) => panic!("{}", e),
        },
        Err(e) => panic!("{}", e),
    };
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    env_logger::init();

    let args = Cli::parse();
    let init_container_name = "rooz-init".to_string();
    let static_data_vol_name = "rooz-static-data".to_string();

    let docker = Docker::connect_with_local_defaults().expect("Docker API connection established");

    let static_data_mount = Mount {
        typ: Some(VOLUME),
        source: Some(static_data_vol_name.to_string()),
        read_only: Some(true),
        ..Default::default()
    };

    match args.command {
        Commands::Open {
            git_ssh_url,
            image,
            user,
            work_dir,
           // ephemeral
        } => {
            let mut image_info = docker.create_image(
                Some(CreateImageOptions::<&str> {
                    from_image: &image,
                    ..Default::default()
                }),
                None,
                None,
            );

            while let Some(l) = image_info.next().await {
                match l {
                    Ok(CreateImageInfo {
                        status: Some(m),
                        progress: p,
                        progress_detail: d,
                        ..
                    }) => {
                        stdout().write_all(&m.as_bytes())?;
                        println!("");
                    }
                    Ok(msg) => panic!("{:?}", msg),
                    Err(e) => panic!("{}", e),
                };
            }

            let inspect = docker.inspect_image(&image).await?;

            let image_user = match &inspect {
                ImageInspect {
                    config: Some(ContainerConfig { user, .. }),
                    ..
                } => user.as_deref(),
                _ => None,
            };

            let user = image_user.or(user.as_deref());

            if let Some(u) = user {
                println!("Inferred user: {}", u);
            }

            let image_id = &inspect.id.as_deref().unwrap();
            println!("Image ID: {}", image_id);

            let home_or_root = user.map_or("/root".to_string(), |u| format!("/home/{}", u));

            let work_dir = work_dir
                .map_or(user.map_or(None, |u| Some(format!("/home/{}", u))), |w| {
                    Some(w)
                });

            let re = Regex::new(r"[^a-zA-Z0-9_.-]")?;
            let container_name = re.replace_all(&git_ssh_url, "-");

            let home_volume_name = &container_name;

            let mut mounts = vec![
                Mount {
                    target: Some(format!("{}/.ssh", home_or_root).to_string()),
                    ..static_data_mount
                },
                Mount {
                    typ: Some(BIND),
                    source: Some("/var/run/docker.sock".to_string()),
                    target: Some("/var/run/docker.sock".to_string()),
                    ..Default::default()
                },
            ];

            //if !ephemeral {
                ensure_volume(&docker, &home_volume_name, "work-data", &container_name).await;

                mounts.push(Mount {
                    typ: Some(VOLUME),
                    source: Some(home_volume_name.to_string()),
                    target: Some(home_or_root.clone()),
                    read_only: Some(false),
                    ..Default::default()
                });
            //}

            let container_id = run(
                &docker,
                &image,
                &image_id,
                user,
                work_dir.as_deref(),
                &container_name,
                Some(mounts),
                false,
                Some(vec!["cat"]),
            )
            .await?;

            let id = container_id.id();

            let clone_work_dir = &git_ssh_url
                .split(&['/'])
                .last()
                .unwrap_or("repo")
                .replace(".git", "")
                .to_string();
            let work_dir = work_dir.map(|d| format!("{}/{}", d.clone(), clone_work_dir.clone()));

            let clone_cmd = inject(
                format!(
                    "ls -la {} > /dev/null || git clone {}",
                    &work_dir.clone().unwrap(),
                    &git_ssh_url
                )
                .as_ref(),
                "clone.sh",
            );

            if let ContainerResult::Created { .. } = container_id {
                exec(
                    &docker,
                    &id,
                    false,
                    None,
                    Some(clone_cmd.iter().map(String::as_str).collect()),
                )
                .await?;
            };

            exec(&docker, id, true, work_dir.as_deref(), Some(vec!["bash"])).await?;

            let remove_options = RemoveContainerOptions {
                force: true,
                ..Default::default()
            };
            docker.remove_container(&id, Some(remove_options)).await?;
        }
        Commands::Init {
            image,
            user,
            work_dir,
        } => {
            ensure_volume(&docker, &static_data_vol_name, "static-data", "default").await;

            let init_ssh = r#"echo "Rooz init"
echo "Running in: $(pwd)"
mkdir -p ~/.ssh
ssh-keyscan -t ed25519 github.com >> ~/.ssh/known_hosts
KEYFILE=~/.ssh/id_ed25519
ls "$KEYFILE.pub" || ssh-keygen -t ed25519 -N '' -f $KEYFILE
#chmod -cv -R 755 $(pwd)
cat "$KEYFILE.pub"
"#;

            let init_entrypoint = inject(&init_ssh, "entrypoint.sh");

            // this needs to be deduped as soon as I know Rust better - start

            let inspect = docker.inspect_image(&image).await?;

            let image_user = match &inspect {
                ImageInspect {
                    config: Some(ContainerConfig { user, .. }),
                    ..
                } => user.as_deref(),
                _ => None,
            };

            let user = image_user.or(user.as_deref());

            if let Some(u) = user {
                println!("Inferred user: {}", u);
            }

            let image_id = &inspect.id.as_deref().unwrap();
            println!("Image ID: {}", image_id);

            let home_or_root = user.map_or("/root".to_string(), |u| format!("/home/{}", u));

            let work_dir = work_dir
                .map_or(user.map_or(None, |u| Some(format!("/home/{}", u))), |w| {
                    Some(w)
                });
            // this needs to be deduped as soon as I know Rust better - end

            run(
                &docker,
                &image,
                "ignore",
                Some("root"),
                work_dir.as_deref(),
                &init_container_name,
                Some(vec![Mount {
                    read_only: Some(false),
                    target: Some(format!("{}/.ssh", home_or_root)),
                    ..static_data_mount.clone()
                }]),
                true,
                Some(vec![
                    "sh",
                    "-c",
                    format!("chown {} {}/.ssh", user.unwrap(), home_or_root).as_ref(),
                ]),
            )
            .await?;

            //SSH INIT
            run(
                &docker,
                &image,
                "ignore",
                user,
                work_dir.as_deref(),
                &init_container_name,
                Some(vec![Mount {
                    read_only: Some(false),
                    target: Some(format!("{}/.ssh", home_or_root)),
                    ..static_data_mount.clone()
                }]),
                true,
                Some(init_entrypoint.iter().map(String::as_str).collect()),
            )
            .await?;
        }
    };
    Ok(())
}
