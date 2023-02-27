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
use clap::Parser;
use futures::stream::StreamExt;
use regex::Regex;
use serde::Deserialize;
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

//TODO: on successfull load clear screen rather than just before cursor - it looks werd if there is a less than a whole
//----- screen of text
//TODO: display better progress when pulling images

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    git_ssh_url: Option<String>,
    #[arg(short, long, default_value = "alpine/git:latest", env = "ROOZ_IMAGE")]
    image: String,
    #[arg(short, long, default_value = "root", env = "ROOZ_USER")]
    user: String,
    #[arg(short, long, default_value = "sh", env = "ROOZ_SHELL")]
    shell: String,
    #[arg(short, long)]
    work_dir: Option<String>,
    #[arg(short, long)]
    temp: bool,
}

#[derive(Debug, Deserialize)]
struct RoozCfg {
    shell: Option<String>,
    image: Option<String>,
    user: Option<String>,
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

enum VolumeResult {
    Created,
    Reused,
}

async fn start_tty(
    docker: &Docker,
    exec_id: &str,
    interactive: bool,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let tty_size = terminal_size()?;
    if let StartExecResults::Attached {
        mut output,
        mut input,
    } = docker.start_exec(exec_id, None).await?
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
                    exec_id,
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
            println!("{}", termion::clear::All);
        };

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
    Ok(())
}

async fn exec(
    docker: &Docker,
    container_id: &str,
    working_dir: Option<&str>,
    cmd: Option<Vec<&str>>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    #[cfg(not(windows))]
    {
        log::debug!("Exec: {:?} in working dir: {:?}", cmd, working_dir);

        Ok(docker
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
            .id)
    }
}

async fn exec_tty(
    docker: &Docker,
    container_id: &str,
    interactive: bool,
    working_dir: Option<&str>,
    cmd: Option<Vec<&str>>,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let exec_id = exec(docker, container_id, working_dir, cmd).await?;
    start_tty(docker, &exec_id, interactive).await
}

async fn exec_output(
    docker: &Docker,
    container_id: &str,
    cmd: Option<Vec<&str>>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let exec_id = exec(docker, container_id, None, cmd).await?;
    if let StartExecResults::Attached { output, .. } = docker.start_exec(&exec_id, None).await? {
        let out = output
            .map(|x| match x {
                Ok(r) => std::str::from_utf8(r.into_bytes().as_ref())
                    .unwrap()
                    .to_string(),
                Err(err) => panic!("{}", err),
            })
            .collect::<Vec<_>>()
            .await
            .join("");

        let trimmed = out.trim();
        Ok(trimmed.to_string())
    } else {
        panic!("Could not start exec");
    }
}

async fn force_remove(
    docker: &Docker,
    container_id: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    Ok(docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await?)
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
    log::debug!(
        "Running {} as {:?} using image {}",
        container_name, user, image
    );

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

async fn ensure_volume(docker: &Docker, name: &str, role: &str, unique_id: &str) -> VolumeResult {
    let static_data_vol_options = CreateVolumeOptions::<&str> {
        name,
        labels: HashMap::from([("dev.rooz.role", role), ("dev.rooz.id", unique_id)]),
        ..Default::default()
    };

    match docker.inspect_volume(&name).await {
        Ok(_) => {
            log::debug!("Reusing an existing {} volume", &name);
            VolumeResult::Reused
        }
        Err(DockerResponseServerError {
            status_code: 404,
            message: _,
        }) => match docker.create_volume(static_data_vol_options).await {
            Ok(v) => {
                log::debug!("Volume created: {:?}", v.name);
                VolumeResult::Created
            }
            Err(e) => panic!("{}", e),
        },
        Err(e) => panic!("{}", e),
    }
}

async fn ensure_image(
    docker: &Docker,
    image: &str,
) -> Result<(String, Option<String>), Box<dyn std::error::Error + 'static>> {
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
                //progress: p,
                //progress_detail: d,
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

    let user = match &inspect {
        ImageInspect {
            config: Some(ContainerConfig { user: Some(u), .. }),
            ..
        } if !u.is_empty() => Some(u),
        _ => None,
    };

    let image_id = &inspect.id.as_deref().unwrap();
    log::debug!("Image ID: {}", image_id);
    Ok((image_id.to_string(), user.map(String::to_string)))
}

fn infer_dirs(user: &str, work_dir: Option<&str>) -> (String, String) {
    let home_or_root = match user {
        "root" => "/root".to_string(),
        u => format!("/home/{}", u.to_string()),
    };

    let work_dir = work_dir.unwrap_or(&home_or_root);

    (home_or_root.to_string(), format!("{}/work", &work_dir))
}

async fn get_uid(
    docker: &Docker,
    image: &str,
    image_id: &str,
    user: &str,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let temp_uid_container_id = run(
        &docker,
        &image,
        &image_id,
        None,
        None,
        "temp-uid",
        None,
        false,
        Some(vec!["cat"]),
    )
    .await?;

    let uid = exec_output(
        &docker,
        &temp_uid_container_id.id(),
        Some(vec!["id", "-u", user]),
    )
    .await?;

    force_remove(&docker, &temp_uid_container_id.id()).await?;
    Ok(uid)
}

async fn chown(
    docker: &Docker,
    image: &str,
    chown_path: &str,
    mount_path: &str,
    volume_name: &str,
    uid: &str,
) -> Result<ContainerResult, Box<dyn std::error::Error + 'static>> {
    log::debug!("Trying to 'chown -R {} {}'", uid, chown_path);
    Ok(run(
        &docker,
        &image,
        "ignore",
        Some("root"),
        None,
        "chown-er",
        Some(vec![Mount {
            typ: Some(VOLUME),
            read_only: Some(false),
            source: Some(volume_name.into()),
            target: Some(mount_path.into()),
            ..Default::default()
        }]),
        true,
        Some(vec![
            "sh",
            "-c",
            format!("chown -R {} {}", uid, chown_path).as_ref(),
        ]),
    )
    .await?)
}

fn get_clone_work_dir(work_dir: &str, git_ssh_url: Option<String>) -> String {
    let clone_work_dir = match git_ssh_url {
        Some(url) => url
            .split(&['/'])
            .last()
            .unwrap_or("repo")
            .replace(".git", "")
            .to_string(),
        None => "".into(),
    };

    log::debug!("Clone dir: {}", &clone_work_dir);

    let work_dir = format!("{}/{}", work_dir, clone_work_dir.clone());

    log::debug!("Full clone dir: {:?}", &work_dir);
    work_dir
}

async fn clone_repo(
    docker: &Docker,
    container_result: ContainerResult,
    git_ssh_url: Option<String>,
    clone_dir: &str,
) -> Result<Option<RoozCfg>, Box<dyn std::error::Error + 'static>> {
    if let Some(url) = git_ssh_url.clone() {
        let cont_id = container_result.id();

        let clone_cmd = inject(
            format!("ls -la {} > /dev/null || git clone {}", &clone_dir, &url).as_ref(),
            "clone.sh",
        );

        if let ContainerResult::Created { .. } = container_result {
            exec_tty(
                &docker,
                &cont_id,
                false,
                None,
                Some(clone_cmd.iter().map(String::as_str).collect()),
            )
            .await?;
        };

        let rooz_cfg = exec_output(
            &docker,
            &cont_id,
            Some(vec![
                "cat",
                format!("{}/{}", clone_dir, ".rooz.toml").as_ref(),
            ]),
        )
        .await?;

        log::debug!("Repo config result: {}", &rooz_cfg);

        Ok(RoozCfg::deserialize(toml::de::Deserializer::new(&rooz_cfg)).ok())
    } else {
        Ok(None)
    }
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

    match args {
        Cli {
            git_ssh_url,
            image,
            shell,
            user,
            work_dir,
            temp,
        } => {
            let orig_shell = shell;
            let orig_work_dir = work_dir;
            let orig_user = user;
            let orig_image = image;

            log::debug!("Work dir (CLAP): {:?}", &orig_work_dir);

            let (orig_image_id, image_user) = ensure_image(&docker, &orig_image).await?;

            let inferred_user = image_user.clone().unwrap_or(orig_user.to_string());
            log::debug!("User (CLAP): {}", &orig_user);
            log::debug!("User (image): {:?}", &image_user.clone());
            log::debug!("User (inferred): {}", &inferred_user);

            let (home_or_root, work_dir2) = infer_dirs(&inferred_user, orig_work_dir.as_deref());

            log::debug!("Home dir (inferred): {}", &home_or_root);
            log::debug!("Work dir (inferred): {:?}", &work_dir2);

            let container_name = match &git_ssh_url {
                Some(url) => {
                    let re = Regex::new(r"[^a-zA-Z0-9_.-]")?;
                    re.replace_all(&url, "-").to_string()
                }
                None => "rooz-work".to_string(),
            };

            let work_volume_name = &container_name;

            let volume_result =
                ensure_volume(&docker, &static_data_vol_name, "static-data", "default").await;

            if let VolumeResult::Created { .. } = volume_result {
                let init_ssh = r#"echo "Rooz init"
echo "Running in: $(pwd)"
mkdir -p ~/.ssh
ssh-keyscan -t ed25519 github.com >> ~/.ssh/known_hosts
KEYFILE=~/.ssh/id_ed25519
ls "$KEYFILE.pub" || ssh-keygen -t ed25519 -N '' -f $KEYFILE
cat "$KEYFILE.pub"
"#;

                let init_entrypoint = inject(&init_ssh, "entrypoint.sh");
                let ssh_path = format!("{}/.ssh", home_or_root);

                chown(
                    &docker,
                    &orig_image,
                    &ssh_path,
                    &ssh_path,
                    &static_data_vol_name,
                    &inferred_user,
                )
                .await?;

                //SSH INIT
                run(
                    &docker,
                    &orig_image,
                    "ignore",
                    Some(&inferred_user),
                    Some(&work_dir2),
                    &init_container_name,
                    Some(vec![Mount {
                        read_only: Some(false),
                        target: Some(ssh_path),
                        ..static_data_mount.clone()
                    }]),
                    true,
                    Some(init_entrypoint.iter().map(String::as_str).collect()),
                )
                .await?;
            };

            let mut mounts = vec![
                Mount {
                    target: Some(format!("{}/.ssh", home_or_root).to_string()),
                    ..static_data_mount.clone()
                },
                Mount {
                    typ: Some(BIND),
                    source: Some("/var/run/docker.sock".to_string()),
                    target: Some("/var/run/docker.sock".to_string()),
                    ..Default::default()
                },
            ];

            if !temp {
                ensure_volume(&docker, &work_volume_name, "work-data", &container_name).await;

                mounts.push(Mount {
                    typ: Some(VOLUME),
                    source: Some(work_volume_name.to_string()),
                    target: Some(work_dir2.clone()),
                    read_only: Some(false),
                    ..Default::default()
                });
            }

            let container_result = run(
                &docker,
                &orig_image,
                &orig_image_id,
                Some(&inferred_user),
                Some(&work_dir2),
                &container_name,
                Some(mounts.clone()),
                false,
                Some(vec!["cat"]),
            )
            .await?;

            let orig_container_id = container_result.id();


            let uid = get_uid(&docker, &orig_image, &orig_image_id, &inferred_user).await?;
            chown(&docker, &orig_image, &work_dir2, &work_dir2, &work_volume_name, &uid).await?;


            let clone_dir = get_clone_work_dir(&work_dir2, git_ssh_url.clone().map(|x|x.to_string()));

            if let Some(RoozCfg {
                image: Some(img),
                user,
                shell,
                ..
            }) = clone_repo(
                &docker,
                container_result.clone(),
                git_ssh_url.clone(),
                &clone_dir,
            )
            .await?
            {
                log::debug!("Image config read from .rooz.toml in the cloned repo");
                let (image_id, image_user) = ensure_image(&docker, &img).await?;

                let (id, clone_dir) = if image_id == orig_image_id {
                    log::debug!("Repo image == Original image. Will reuse container");
                    (orig_container_id.to_string(), work_dir2)
                } else {
                    log::debug!("Replacing container using new image {}", image_id);

                    force_remove(&docker, &orig_container_id).await?;

                    let inferred_user = user.or(image_user).unwrap_or(orig_user.to_string());
                    let (home_or_root, work_dir) =
                        infer_dirs(&inferred_user, orig_work_dir.as_deref());

                    let clone_dir = get_clone_work_dir(&work_dir, git_ssh_url.clone());

                    //deduplicate the code later on - start
                    let mut mounts = vec![
                        Mount {
                            target: Some(format!("{}/.ssh", home_or_root).to_string()),
                            ..static_data_mount.clone()
                        },
                        Mount {
                            typ: Some(BIND),
                            source: Some("/var/run/docker.sock".to_string()),
                            target: Some("/var/run/docker.sock".to_string()),
                            ..Default::default()
                        },
                    ];

                    if !temp {
                        ensure_volume(&docker, &work_volume_name, "work-data", &container_name)
                            .await;

                        let uid = get_uid(&docker, &img, &image_id, &inferred_user).await?;
                        chown(&docker, &orig_image, &work_dir, &work_dir, &work_volume_name, &uid).await?;

                        mounts.push(Mount {
                            typ: Some(VOLUME),
                            source: Some(work_volume_name.to_string()),
                            target: Some(work_dir.to_string()),
                            read_only: Some(false),
                            ..Default::default()
                        });
                    }
                    //deduplicate the code later on - end

                    let r = run(
                        &docker,
                        &img,
                        &image_id,
                        Some(&inferred_user),
                        Some(&clone_dir),
                        &container_name,
                        Some(mounts.clone()),
                        false,
                        Some(vec!["cat"]),
                    )
                    .await?;

                    let id = &r.id().to_string();
                    (id.clone(), clone_dir)
                };

                let sh = shell.or(Some(orig_shell)).unwrap();
                exec_tty(&docker, &id, true, Some(&clone_dir), Some(vec![&sh])).await?;
                force_remove(&docker, &id).await?;
            } else {
                // free-style container or no .rooz.toml

                exec_tty(
                    &docker,
                    orig_container_id,
                    true,
                    Some(&clone_dir),
                    Some(vec![&orig_shell]),
                )
                .await?;

                force_remove(&docker, &orig_container_id).await?;
            }
        }
    };
    Ok(())
}
