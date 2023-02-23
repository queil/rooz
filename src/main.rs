use base64::{engine::general_purpose, Engine as _};
use bollard::container::LogOutput::Console;
use bollard::container::{Config, CreateContainerOptions, LogsOptions, StartContainerOptions};
use bollard::errors::Error::DockerResponseServerError;
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::MountTypeEnum::{BIND, VOLUME};
use bollard::models::{HostConfig, Mount};
use bollard::volume::CreateVolumeOptions;
use bollard::Docker;
use clap::{Parser, Subcommand};
use futures::stream::{StreamExt, TryStreamExt};
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

//TODO: CLI: rooz into [repo] [image] (?--transient)
//TODO: tinker with different workflows: i.e. ephemeral - clone-develop-destroy
//TODO: configuration (allow using custom images)
//TODO: for POC we run as root, then let's figure out how we can reliably ensure a non-root user regardless of the Linux distro in all launched containers

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
        #[arg(short, long, default_value = "alpine/git:latest")]
        image: String,
        #[arg(short, long)]
        user: Option<String>,
        #[arg(short, long)]
        work_dir: Option<String>,
        #[arg(short, long)]
        emphemeral: bool,
    },
    Init {},
}

async fn exec(
    docker: &Docker,
    container_id: &str,
    interactive: bool, // this is a hack only needed to avoid the garbage bytes written to TTY when opening a new exec
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

            docker
                .resize_exec(
                    &exec,
                    ResizeExecOptions {
                        height: tty_size.1,
                        width: tty_size.0,
                    },
                )
                .await?;

            // set stdout in raw mode so we can do tty stuff
            let stdout = stdout();
            let mut stdout = stdout.lock().into_raw_mode()?;

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
    user: Option<&str>,
    work_dir: Option<&str>,
    container_name: &str,
    mounts: Option<Vec<Mount>>,
    log: bool,
    entrypoint: Option<Vec<&str>>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    println!("Running {}", container_name);

    let options = CreateContainerOptions {
        name: container_name, // this needs to be random
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

    let container_id = docker
        .create_container(Some(options.clone()), config.clone())
        .await?
        .id;

    docker
        .start_container(&container_id, None::<StartContainerOptions<String>>)
        .await?;

    // attach to container
    //        let AttachContainerResults {
    //            mut output,
    //           ..
    //        } = docker
    //            .attach_container(
    //                &container_id,
    //                Some(AttachContainerOptions::<String> {
    //                    stdout: Some(true),
    //                    stderr: Some(true),
    //                    stdin: Some(false),
    //                    stream: Some(true),
    //                    ..Default::default()
    //                }),
    //            )
    //            .await?;
    //        // pipe stdin into the docker attach stream input
    ////        spawn(async move {
    ////            let mut stdin = async_stdin().bytes();
    ////            loop {
    ////                if let Some(Ok(byte)) = stdin.next() {
    ////                    input.write(&[byte]).await.ok();
    ////                } else {
    ////                    sleep(Duration::from_nanos(10)).await;
    ////                }
    ////            }
    ////        });
    //
    //        // set stdout in raw mode so we can do tty stuff
    //        let stdout = stdout();
    //        let mut stdout = stdout.lock().into_raw_mode()?;
    //
    //        // pipe docker attach output into stdout
    //        while let Some(Ok(output)) = output.next().await {
    //            stdout
    //                .write_all(output.into_bytes().as_ref())?;
    //
    //            stdout.flush()?;
    //        }

    if log {
        let log_options = LogsOptions::<String> {
            stdout: true,
            follow: true,
            ..Default::default()
        };

        let mut stream = docker.logs(&container_name, Some(log_options));

        while let Some(l) = stream.next().await {
            match l {
                Ok(Console { message: m }) => stdout().write_all(&m).expect("Write to stdout"),
                Ok(msg) => panic!("{}", msg),
                Err(e) => panic!("{}", e),
            };
        }
    }

    Ok(container_id.clone())
}

fn inject(script: &str) -> Vec<String> {
    vec![
          "sh".to_string(),
          "-c".to_string(),
            format!(
                    "echo '{}' | base64 -d > /tmp/entrypoint.sh && chmod +x /tmp/entrypoint.sh && /tmp/entrypoint.sh",
                general_purpose::STANDARD.encode(script.trim())
            )
        ]
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    let args = Cli::parse();
    let init_image = "alpine/git:latest".to_string();
    let init_container_name = "rooz-init".to_string();
    let static_data_vol_name = "rooz-static-data".to_string();
    // mounting SSH for the init - we can mount as RW and let root initialize the keys directly in the volume
    // mounting SSH for the user - we can mount as RO and try this overlay trick:
    // docker exec -d myenv /sbin/mount -t overlay overlay -o lowerdir=/mnt/rocode,upperdir=/mycode,workdir=/mnt/code-workdir /mycode
    let static_data_mount_path = "/mnt/rooz/static";

    let docker = Docker::connect_with_local_defaults().expect("Docker API connection established");

    let static_data_mount = Mount {
        typ: Some(VOLUME),
        source: Some(static_data_vol_name.to_string()),
        read_only: Some(true),
        target: Some(static_data_mount_path.to_string()),
        ..Default::default()
    };

    let proj_init_container_name = "rooz-proj-init";

    match args.command {
        Commands::Open {
            git_ssh_url,
            image,
            user,
            work_dir,
            emphemeral: _,
        } => {
            docker
                .create_image(
                    Some(CreateImageOptions::<&str> {
                        from_image: &image,
                        ..Default::default()
                    }),
                    None,
                    None,
                )
                .try_collect::<Vec<_>>()
                .await?;

            //

            //         #mount -t overlay overlay -o lowerdir=/mnt/rooz/.ssh,upperdir=/home/queil/.ssh,workdir=/mnt/code-workdir /home/queil/.ssh

            let user = user.as_deref();

            let home_or_root = user.map_or("/root".to_string(), |u| format!("/home/{}", u));

            let init_ssh_overlay = format!(
                r#"mkdir -p {}/.ssh && \
cp "{}/.ssh" -R {} && \
chmod 400 "{}/.ssh/id_ed25519"
cat
"#,
                home_or_root, static_data_mount_path, home_or_root, home_or_root
            );

            let entryp = inject(&init_ssh_overlay);

            println!("{:?}", &entryp);

            let work_dir = work_dir
                .map_or(user.map_or(None, |u| Some(format!("/home/{}", u))), |w| {
                    Some(w)
                });

            let container_id = run(
                &docker,
                &image,
                user,
                work_dir.as_deref(),
                &proj_init_container_name,
                Some(vec![
                    static_data_mount,
                    Mount {
                        typ: Some(BIND),
                        source: Some("/var/run/docker.sock".to_string()),
                        target: Some("/var/run/docker.sock".to_string()),
                        ..Default::default()
                    },
                ]),
                false,
                Some(entryp.iter().map(String::as_str).collect()),
                //Some(vec!["cat"])
            )
            .await?;

            exec(
                &docker,
                &container_id,
                false,
                Some(vec!["git", "clone", &git_ssh_url]),
            )
            .await?;

            exec(&docker, &container_id, true, Some(vec!["bash"])).await?;
        }
        Commands::Init {} => {
            let static_data_vol_options = CreateVolumeOptions::<&str> {
                name: &static_data_vol_name,
                labels: HashMap::from([("dev.rooz.role", "static-data")]),
                ..Default::default()
            };

            match docker.inspect_volume(&static_data_vol_name).await {
                Ok(_) => println!("Reusing an existing ssh-keys volume"),
                Err(DockerResponseServerError {
                    status_code: 404,
                    message: _,
                }) => match docker.create_volume(static_data_vol_options).await {
                    Ok(v) => println!("Volume created: {:?}", v.name),
                    Err(e) => panic!("{}", e),
                },
                Err(e) => panic!("{}", e),
            };

            // 755 for the files so they may be shared between containers regardless which user runs them
            // --
            let init_ssh = r#"echo "Rooz init"
echo "Running in: $(pwd)"
mkdir -p .ssh
ssh-keyscan -t ed25519 github.com >> .ssh/known_hosts
KEYFILE=.ssh/id_ed25519
ls "$KEYFILE.pub" || ssh-keygen -t ed25519 -N '' -f $KEYFILE
chmod -cv -R 755 $(pwd)
cat "$KEYFILE.pub"
"#;

            let init_entrypoint = inject(&init_ssh);

            //SSH INIT
            run(
                &docker,
                &init_image,
                None,
                Some(&static_data_mount_path),
                &init_container_name,
                Some(vec![Mount {
                    read_only: Some(false),
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
