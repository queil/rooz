use base64::{engine::general_purpose, Engine as _};
use bollard::container::LogOutput::{self, Console};
use bollard::container::{
    Config, CreateContainerOptions, LogsOptions, RemoveContainerOptions, StartContainerOptions, ListContainersOptions,
};
use bollard::errors::Error::{self, DockerResponseServerError};
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::MountTypeEnum::{BIND, VOLUME};
use bollard::models::{CreateImageInfo, HostConfig, Mount};
use bollard::service::{ContainerConfig, ContainerInspectResponse, ImageInspect, ContainerSummary};
use bollard::volume::{CreateVolumeOptions, ListVolumesOptions, RemoveVolumeOptions};
use bollard::Docker;
use clap::Parser;
use futures::stream::StreamExt;
use futures::Stream;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{stdout, Read, Write};
use std::path::Path;
use std::time::Duration;
#[cfg(not(windows))]
use termion::raw::IntoRawMode;
#[cfg(not(windows))]
use termion::{async_stdin, terminal_size};
use tokio::io::AsyncWriteExt;
use tokio::task::spawn;
use tokio::time::sleep;

//TODO: display better progress when pulling images

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    git_ssh_url: Option<String>,
    #[arg(
        short,
        long,
        default_value = "alpine/git:latest",
        env = "ROOZ_IMAGE"
    )]
    image: String,
    #[arg(short, long, default_value = "sh", env = "ROOZ_SHELL")]
    shell: String,
    #[arg(short, long)]
    work_dir: Option<String>,
    #[arg(short, long)]
    temp: bool,
    #[arg(short, long)]
    prune: bool
}

#[derive(Debug, Deserialize)]
struct RoozCfg {
    shell: Option<String>,
    image: Option<String>,
    caches: Option<Vec<String>>
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

async fn collect(
    stream: impl Stream<Item = Result<LogOutput, Error>>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let out = stream
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
}

async fn exec_output(
    docker: &Docker,
    container_id: &str,
    cmd: Option<Vec<&str>>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let exec_id = exec(docker, container_id, None, cmd).await?;
    if let StartExecResults::Attached { output, .. } = docker.start_exec(&exec_id, None).await? {
        collect(output).await
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
    entrypoint: Option<Vec<&str>>,
) -> Result<ContainerResult, Box<dyn std::error::Error + 'static>> {
    log::debug!(
        "Running {} as {:?} using image {}",
        container_name,
        user,
        image
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
                labels: Some(HashMap::from([("dev.rooz", "true")])),
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

    Ok(container_id.clone())
}

async fn container_logs_to_stdout(
    docker: &Docker,
    container_name: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
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
    Ok(())
}

async fn container_logs(
    docker: &Docker,
    container_name: &str,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let log_options = LogsOptions::<String> {
        stdout: true,
        follow: true,
        ..Default::default()
    };

    let output = docker.logs(&container_name, Some(log_options));
    collect(output).await
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

async fn ensure_volume(docker: &Docker, name: &str, role: &str) -> VolumeResult {
    let static_data_vol_options = CreateVolumeOptions::<&str> {
        name,
        labels: HashMap::from([("dev.rooz", "true"), ("dev.rooz.role", role)]),
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
            format!(
                "ls -la {} > /dev/null || git clone {} {}",
                &clone_dir, &url, &clone_dir
            )
            .as_ref(),
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

fn to_safe_id(dirty: &str) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let re = Regex::new(r"[^a-zA-Z0-9_.-]").unwrap();
    Ok(re.replace_all(&dirty, "-").to_string())
}

const ROOZ_SSH_KEY_VOLUME_NAME: &'static str = "rooz-ssh-key-vol";
const ROOZ_ETC_VOLUME_NAME: &'static str = "rooz-ect-vol";

async fn init_ssh_key(
    docker: &Docker,
    image: &str,
    user: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let init_ssh = format!(
        r#"echo "Rooz init"
echo "Running in: $(pwd)"
mkdir -p /tmp/.ssh
ssh-keyscan -t ed25519 github.com 140.82.121.4 140.82.121.3 >> /tmp/.ssh/known_hosts
KEYFILE=/tmp/.ssh/id_ed25519
ls "$KEYFILE.pub" || ssh-keygen -t ed25519 -N '' -f $KEYFILE -C rooz-access-key
cat "$KEYFILE.pub"
chown -R {} /tmp/.ssh
"#,
        &user
    );

    let init_entrypoint = inject(&init_ssh, "entrypoint.sh");

    let result = run(
        &docker,
        &image,
        "ignore",
        Some("root"),
        None,
        "rooz-init-ssh",
        Some(vec![Mount {
            typ: Some(VOLUME),
            read_only: Some(false),
            source: Some(ROOZ_SSH_KEY_VOLUME_NAME.into()),
            target: Some("/tmp/.ssh".into()),
            ..Default::default()
        }]),
        Some(init_entrypoint.iter().map(String::as_str).collect()),
    )
    .await?;

    container_logs_to_stdout(docker, result.id()).await?;

    Ok(())
}

fn safe_volume_name(
    path: &str,
    unique_id: Option<&str>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let safe_id = to_safe_id(match path {
        "~/" => "home",
        "~/work" => "work",
        s => s,
    })?;

    let vol_name = match unique_id {
        Some(id) => format!("rooz-{}-{}", to_safe_id(&id.to_string())?, &safe_id),
        None => format!("rooz-{}", &safe_id),
    };
    Ok(vol_name)
}

async fn ensure_user(
    docker: &Docker,
    image: &str,
    uid: &str,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let ensure_user = format!(
        r#"NEWUID={}
LOG=/tmp/ensure_user.log
USER_NAME=$(id -u $NEWUID >$LOG 2>&1 && echo $(id -u $NEWUID -n))
if [ -z "${{USER_NAME}}" ]
then
  USER_NAME=rooz_user
  ROOZ_UID=$(id -u $USER_NAME >$LOG 2>&1 && echo $(id -u $USER_NAME))

  if [ -z "${{ROOZ_UID}}" ]
  then
    (adduser $USER_NAME --uid $NEWUID --no-create-home --disabled-password -gecos "" >$LOG 2>&1 || \
     useradd $USER_NAME --uid $NEWUID --no-create-home --no-log-init >$LOG 2>&1) && \
      mkdir /home/$USER_NAME >$LOG 2>&1 && \
      chown $NEWUID /home/$USER_NAME
  else
    usermod $USER_NAME -u $NEWUID >$LOG 2>&1
  fi
fi
echo $USER_NAME
"#,
        uid
    );

    let init_entrypoint = inject(&ensure_user, "entrypoint.sh");

    ensure_volume(&docker, ROOZ_ETC_VOLUME_NAME.into(), "etc").await;

    let result = run(
        &docker,
        &image,
        "ignore",
        Some("root"),
        None,
        "rooz-init-user",
        Some(vec![Mount {
            typ: Some(VOLUME),
            source: Some(ROOZ_ETC_VOLUME_NAME.into()),
            target: Some("/etc".into()),
            read_only: Some(false),
            ..Default::default()
        }]),
        Some(init_entrypoint.iter().map(String::as_str).collect()),
    )
    .await?;

    let user = container_logs(docker, result.id()).await?;

    log::debug!("User name: {}", &user);
    Ok(user)
}

async fn ensure_vol_access(
    docker: &Docker,
    image: &str,
    uid: &str,
    mount: Mount,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let target_dir = mount.clone().target.unwrap();
    let init_entrypoint = inject(
        format!(r#"chown {} {}"#, uid, target_dir).as_str(),
        "entrypoint.sh",
    );
    let result = run(
        &docker,
        &image,
        "ignore",
        Some("root"),
        Some(&target_dir),
        "rooz-vol-access",
        Some(vec![mount.clone()]),
        Some(init_entrypoint.iter().map(String::as_str).collect()),
    )
    .await?;

    container_logs_to_stdout(docker, result.id()).await?;

    Ok(())
}

async fn ensure_mounts(
    docker: &Docker,
    image: &str,
    uid: &str,
    paths: Vec<String>,
    is_ephemeral: bool,
    unique_id: Option<&str>,
    home_dir: &str,
) -> Result<Vec<Mount>, Box<dyn std::error::Error + 'static>> {
    let mut mounts = vec![
        Mount {
            typ: Some(BIND),
            source: Some("/var/run/docker.sock".to_string()),
            target: Some("/var/run/docker.sock".to_string()),
            ..Default::default()
        },
        Mount {
            typ: Some(VOLUME),
            source: Some(ROOZ_SSH_KEY_VOLUME_NAME.into()),
            target: Some(
                Path::new(home_dir)
                    .join(".ssh")
                    .to_string_lossy()
                    .to_string(),
            ),
            read_only: Some(true),
            ..Default::default()
        },
        Mount {
            typ: Some(VOLUME),
            source: Some(ROOZ_ETC_VOLUME_NAME.into()),
            target: Some("/etc".into()),
            read_only: Some(true),
            ..Default::default()
        },
    ];

    if is_ephemeral {
        return Ok(mounts.clone());
    }

    for p in paths {
        let role = match p.as_str() {
            "~/" => "home",
            "~/work" => "work",
            _ => "cache",
        };

        let vol_name = safe_volume_name(&p, unique_id)?;

        ensure_volume(&docker, &vol_name, role).await;

        let mount = Mount {
            typ: Some(VOLUME),
            source: Some(vol_name.into()),
            target: Some(p.replace("~", &home_dir)),
            read_only: Some(false),
            ..Default::default()
        };

        ensure_vol_access(docker, image, uid, mount.clone()).await?;

        mounts.push(mount);
    }

    Ok(mounts.clone())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    env_logger::init();

    let args = Cli::parse();
    let docker = Docker::connect_with_local_defaults().expect("Docker API connection established");

    match args {
        Cli {
            git_ssh_url,
            image,
            shell,
            work_dir,
            temp,
            prune
        } => {
            if prune {

                let ls_container_options = ListContainersOptions {
                    all: true,
                    filters: HashMap::from([
                        ("label", vec!["dev.rooz"])
                    ]),
                    ..Default::default()
                };
                for cs in docker.list_containers(Some(ls_container_options)).await? {

                    if let ContainerSummary{id: Some(id), ..} = cs {
                        log::debug!("Force remove container: {}", &id);
                        force_remove(&docker, &id).await?
                    }
                }

                let ls_vol_options = ListVolumesOptions{
                    filters: HashMap::from([
                        ("label", vec!["dev.rooz"])
                    ]),
                    ..Default::default()
                };

                if let Some(volumes) = docker.list_volumes(Some(ls_vol_options)).await?.volumes {

                    let rm_vol_options = RemoveVolumeOptions {
                        force: true,
                        ..Default::default()
                    };

                    for v in volumes {
                        log::debug!("Force remove volume: {}", &v.name);
                        docker.remove_volume(&v.name, Some(rm_vol_options)).await?
                    }
                }
            }

            let orig_shell = shell;
            let orig_work_dir = work_dir;
            let orig_uid = "1000".to_string();
            let orig_image = image;

            log::debug!("Work dir (CLAP): {:?}", &orig_work_dir);

            let (orig_image_id, _) = ensure_image(&docker, &orig_image).await?;
            log::debug!("User (CLAP): {}", &orig_uid);

            let container_name = match &git_ssh_url {
                Some(url) => {
                    let re = Regex::new(r"[^a-zA-Z0-9_.-]")?;
                    re.replace_all(&url, "-").to_string()
                }
                None => "rooz-work".to_string(),
            };

            let ssh_key_vol_result =
                ensure_volume(&docker, ROOZ_SSH_KEY_VOLUME_NAME.into(), "ssh-key").await;

            if let VolumeResult::Created { .. } = ssh_key_vol_result {
                init_ssh_key(&docker, &orig_image_id, &orig_uid).await?;
            };

            let user = ensure_user(&docker, &orig_image, &orig_uid).await?;

            let home_dir = format!("/home/{}", &user);
            let work_dir = format!("{}/work", &home_dir);

            let vol_paths = vec![home_dir.clone(), work_dir.clone()];

            let mounts = ensure_mounts(
                &docker,
                &orig_image,
                &orig_uid,
                vol_paths.clone(),
                temp,
                git_ssh_url.as_deref(),
                &home_dir.clone(),
            )
            .await?;

            let container_result = run(
                &docker,
                &orig_image,
                &orig_image_id,
                Some(&orig_uid),
                None,
                &container_name,
                Some(mounts.clone()),
                Some(vec!["cat"]),
            )
            .await?;

            let orig_container_id = container_result.id();

            let clone_dir =
                get_clone_work_dir(&work_dir, git_ssh_url.clone().map(|x| x.to_string()));

            if let Some(RoozCfg {
                image: Some(img),
                shell,
                caches,
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
                let (image_id, _) = ensure_image(&docker, &img).await?;

                let (id, clone_dir) = if image_id == orig_image_id {
                    log::debug!("Repo image == Original image. Will reuse container");
                    (orig_container_id.to_string(), work_dir.into())
                } else {
                    log::debug!("Replacing container using new image {}", image_id);

                    force_remove(&docker, &orig_container_id).await?;

                    let paths = if let Some(caches) = &caches {
                        let mut paths = vol_paths.clone();
                        paths.extend_from_slice(caches.clone().as_slice());
                        paths
                    } else {
                        vol_paths.clone()
                    };

                    ensure_mounts(
                        &docker,
                        &img,
                        &orig_uid,
                        paths,
                        temp,
                        git_ssh_url.as_deref(),
                        &home_dir,
                    )
                    .await?;

                    let r = run(
                        &docker,
                        &img,
                        &image_id,
                        Some(&orig_uid),
                        Some(&clone_dir),
                        &container_name,
                        Some(mounts.clone()),
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
