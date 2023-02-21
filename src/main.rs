use base64::{engine::general_purpose, Engine as _};
use bollard::container::LogOutput::Console;
use bollard::container::{
    AttachContainerOptions, AttachContainerResults, Config, CreateContainerOptions, LogsOptions,
    RemoveContainerOptions, StartContainerOptions,
};
use bollard::errors::Error::{self, DockerResponseServerError};
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::MountTypeEnum::VOLUME;
use bollard::models::{HostConfig, Mount};
use bollard::volume::CreateVolumeOptions;
use bollard::Docker;
use clap::{Parser, Subcommand};
use futures::stream::StreamExt;
use std::collections::HashMap;
use std::io::{stdout, Read, Write};
use std::time::Duration;
#[cfg(not(windows))]
use termion::async_stdin;
#[cfg(not(windows))]
use termion::raw::IntoRawMode;
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
        #[arg(short, long)]
        image: Option<String>,
        #[arg(short, long)]
        emphemeral: bool,
    },
}

async fn run(
    docker: &Docker,
    image: &str,
    container_name: &str,
    ssh_keys_volume_name: &str,
    entrypoint: Option<Vec<&str>>,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    println!("Running {}", container_name);

    let options = CreateContainerOptions {
        name: container_name, // this needs to be random
        platform: None,
    };

    let host_config = HostConfig {
        auto_remove: Some(true),
        mounts: Some(vec![Mount {
            typ: Some(VOLUME),
            source: Some(ssh_keys_volume_name.to_string()),
            read_only: Some(false),
            target: Some("/root/.ssh".to_string()),
            ..Default::default()
        }]),
        ..Default::default()
    };

    let config = Config {
        image: Some(image),
        entrypoint,
        working_dir: Some("/build"),
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
    //    {
    //        Ok(r) => r, //add logging
    //        Err(DockerResponseServerError {
    //            status_code: 409,
    //            message: _,
    //        }) => {
    //            let remove_options = RemoveContainerOptions {
    //                force: true,
    //                ..Default::default()
    //            };
    //            docker
    //                .remove_container(container_name, Some(remove_options))
    //                .await?;
    //            docker
    //                .create_container(Some(options), config)
    //                .await?
    //        }
    //        Err(e) => panic!("{}", e),
    //    };

    docker
        .start_container(&container_id, None::<StartContainerOptions<String>>)
        .await?;

    let AttachContainerResults {
        mut output,
        mut input,
    } = docker
        .attach_container(
            &container_id,
            Some(AttachContainerOptions::<String> {
                stdout: Some(true),
                stderr: Some(true),
                stdin: Some(true),
                stream: Some(true),
                ..Default::default()
            }),
        )
        .await?;
    // pipe stdin into the docker attach stream input
    spawn(async move {
        let mut stdin = async_stdin().bytes();
        loop {
            if let Some(Ok(byte)) = stdin.next() {
                input.write(&[byte]).await.ok();
            } else {
                sleep(Duration::from_nanos(10)).await;
            }
        }
    });

    // set stdout in raw mode so we can do tty stuff
    let stdout = stdout();
    let mut stdout = stdout.lock().into_raw_mode()?;

    // pipe docker attach output into stdout
    while let Some(Ok(output)) = output.next().await {
        stdout
            .write_all(output.into_bytes().as_ref())?;

        stdout.flush()?;
    }
    Ok(())

    //    #[cfg(not(windows))]

    //    let log_options = LogsOptions::<String> {
    //        stdout: true,
    //        follow: true,
    //        ..Default::default()
    //    };

    //    let mut stream = docker.logs(container_name, Some(log_options));
    //
    //    while let Some(l) = stream.next().await {
    //        match l {
    //            Ok(Console { message: m }) => stdout().write_all(&m).expect("Write to stdout"),
    //            Ok(msg) => panic!("{}", msg),
    //            Err(e) => panic!("{}", e),
    //        };
    //    }
}

#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let init_image = "alpine/git:latest".to_string();
    let init_container_name = "rooz-init".to_string();
    let ssh_keys_volume_name = "rooz-ssh-keys".to_string();

    let docker = Docker::connect_with_local_defaults().expect("Docker API connection established");

    // volumes
    let ssh_keys_vol_options = CreateVolumeOptions::<String> {
        name: ssh_keys_volume_name.clone(),
        labels: HashMap::from([("dev.rooz.role".to_string(), "ssh-keys".to_string())]),
        ..Default::default()
    };

    match docker.inspect_volume(&ssh_keys_volume_name).await {
        Ok(_) => println!("Reusing an existing ssh-keys volume"),
        Err(DockerResponseServerError {
            status_code: 404,
            message: _,
        }) => match docker.create_volume(ssh_keys_vol_options).await {
            Ok(v) => println!("Volume created: {:?}", v.name),
            Err(e) => panic!("{}", e),
        },
        Err(e) => panic!("{}", e),
    };

    let init_ssh = general_purpose::STANDARD.encode(
        r#"
    ssh-keyscan -t ed25519 github.com >> /root/.ssh/known_hosts
    KEYFILE='/root/.ssh/id_ed25519'
    ls "$KEYFILE.pub" || ssh-keygen -t ed25519 -N '' -f $KEYFILE
    cat "$KEYFILE.pub"
    "#,
    );

    fn inject(script: &str) -> Vec<String> {
        vec![
          "sh".to_string(),
          "-c".to_string(),
            format!(
                    "echo '{}' | base64 -d > /entrypoint.sh && chmod +x /entrypoint.sh && /entrypoint.sh",
                script
            )
        ]
    }

    let init_entrypoint = inject(&init_ssh);

    //    let _d = run(
    //        &docker,
    //        &init_image,
    //        &init_container_name,
    //        &ssh_keys_volume_name,
    //        Some(init_entrypoint.iter().map(|x|x.as_ref()).collect()),
    //    )
    //    .await;

    let proj_init_container_name = "rooz-proj-init";

    match args.command {
        Commands::Open {
            git_ssh_url,
            image,
            emphemeral: _,
        } => {
            let clone_and_pause = general_purpose::STANDARD.encode(format!(
                r#"
    git clone "{}" && sh
    "#,
                git_ssh_url
            ));

            let image2 = &image.unwrap_or(init_image.clone());
            let entryp = inject(&clone_and_pause);
            run(
                &docker,
                image2,
                &proj_init_container_name,
                &ssh_keys_volume_name,
                Some(entryp.iter().map(|x| x.as_ref()).collect()),
            )
            .await;

            //
            //            let create_exec_options = CreateExecOptions::<&str> {
            //                working_dir: Some("/build"),
            //                attach_stdin: Some(true),
            //                attach_stdout: Some(true),
            //                attach_stderr: Some(true),
            //                tty: Some(true),
            //                cmd: Some(vec!["sh"]),
            //                ..Default::default()
            //            };
            //
            //            let result = docker
            //                .create_exec(&proj_init_container_name, create_exec_options)
            //                .await
            //                .expect("Created exec");
            //
            //            println!("After create exec");
            //            if let StartExecResults::Attached {
            //                mut output,
            //                mut input,
            //            } = docker
            //                .start_exec(&result.id, None)
            //                .await
            //                .expect("Start exec")
            //            {
            //
            //            } else {
            //                unreachable!();
            //            }
        }
    };
}

//insteadof logging and/or exec'ing just do the same TTY trick as with the exec
