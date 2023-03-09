use base64::{engine::general_purpose, Engine as _};
use bollard::container::LogOutput::{self, Console};
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions,
};
use bollard::errors::Error::{self, DockerResponseServerError};
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::MountTypeEnum::{BIND, VOLUME};
use bollard::models::{CreateImageInfo, HostConfig, Mount};
use bollard::service::{
    ContainerConfig, ContainerInspectResponse, ContainerSummary, ImageInspect, Volume,
};
use bollard::volume::{CreateVolumeOptions, ListVolumesOptions, RemoveVolumeOptions};
use bollard::Docker;
use clap::Parser;
use futures::stream::StreamExt;
use futures::Stream;
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
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

fn random_suffix(prexif: &str) -> String {
    let suffix: String = thread_rng()
        .sample_iter(&Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    format!("{}-{}", prexif, suffix)
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    git_ssh_url: Option<String>,
    #[arg(short, long, default_value = "bitnami/git:latest", env = "ROOZ_IMAGE")]
    image: String,
    #[arg(short, long, default_value = "bash", env = "ROOZ_SHELL")]
    shell: String,
    #[arg(short, long, default_value = "rooz_user", env = "ROOZ_USER")]
    user: String,
    #[arg(short, long)]
    prune: bool,
}

#[derive(Debug, Deserialize)]
struct RoozCfg {
    shell: Option<String>,
    image: Option<String>,
    caches: Option<Vec<String>>,
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

#[derive(Debug, Clone)]
enum RoozVolumeSharing {
    Shared,
    Exclusive { key: String },
}

#[derive(Debug, Clone)]
enum RoozVolumeRole {
    Home,
    Work,
    Cache,
    Git,
}

impl RoozVolumeRole {
    pub fn as_str(&self) -> &str {
        match self {
            RoozVolumeRole::Home => "home",
            RoozVolumeRole::Work => "work",
            RoozVolumeRole::Cache => "cache",
            RoozVolumeRole::Git => "git",
        }
    }
}

#[derive(Debug, Clone)]
struct RoozVolume {
    path: String,
    role: RoozVolumeRole,
    sharing: RoozVolumeSharing,
}

impl RoozVolume {
    pub fn safe_volume_name(&self) -> Result<String, Box<dyn std::error::Error + 'static>> {
        let safe_id = to_safe_id(self.role.as_str())?;

        let vol_name = match self {
            RoozVolume {
                sharing: RoozVolumeSharing::Exclusive { key },
                ..
            } => format!("rooz-{}-{}", to_safe_id(&key)?, &safe_id),
            RoozVolume {
                path: p,
                sharing: RoozVolumeSharing::Shared,
                role: RoozVolumeRole::Cache,
                ..
            } => format!("rooz-{}-{}", to_safe_id(&p)?, &safe_id),
            RoozVolume { .. } => format!("rooz-{}", &safe_id),
        };
        Ok(vol_name)
    }
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
    reason: &str,
    docker: &Docker,
    container_id: &str,
    working_dir: Option<&str>,
    user: Option<&str>,
    cmd: Option<Vec<&str>>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    #[cfg(not(windows))]
    {
        log::debug!(
            "[{}] exec: {:?} in working dir: {:?}",
            reason,
            cmd,
            working_dir
        );

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
                    user,
                    ..Default::default()
                },
            )
            .await?
            .id)
    }
}

async fn exec_tty(
    reason: &str,
    docker: &Docker,
    container_id: &str,
    interactive: bool,
    working_dir: Option<&str>,
    user: Option<&str>,
    cmd: Option<Vec<&str>>,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let exec_id = exec(reason, docker, container_id, working_dir, user, cmd).await?;
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
    reason: &str,
    docker: &Docker,
    container_id: &str,
    user: Option<&str>,
    cmd: Option<Vec<&str>>,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let exec_id = exec(reason, docker, container_id, None, user, cmd).await?;
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
    reason: &str,
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
        "[{}]: running {} as {:?} using image {}",
        &reason,
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
    let create_vol_options = CreateVolumeOptions::<&str> {
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
        }) => match docker.create_volume(create_vol_options).await {
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
    let img_chunks = &image.split(':').collect::<Vec<&str>>();
    let mut image_info = docker.create_image(
        Some(CreateImageOptions::<&str> {
            from_image: &image,
            tag: match img_chunks.len() {
                2 => img_chunks[1],
                _ => "latest",
            },
            ..Default::default()
        }),
        None,
        None,
    );

    while let Some(l) = image_info.next().await {
        match l {
            Ok(CreateImageInfo {
                id,
                status: Some(m),
                progress: p,
                ..
            }) => {
                if let Some(id) = id {
                    stdout().write_all(&id.as_bytes())?;
                } else {
                    println!("");
                }
                print!(" ");
                stdout().write_all(&m.as_bytes())?;
                print!(" ");
                if let Some(x) = p {
                    stdout().write_all(&x.as_bytes())?;
                };
                print!("\r");
            }
            Ok(msg) => panic!("{:?}", msg),
            Err(e) => panic!("{}", e),
        };
    }

    println!("");
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

fn get_clone_dir(root_dir: &str, git_ssh_url: Option<String>) -> String {
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

    let work_dir = format!("{}/{}", root_dir, clone_work_dir.clone());

    log::debug!("Full clone dir: {:?}", &work_dir);
    work_dir
}

async fn git_volume(
    docker: &Docker,
    uid: &str,
    url: &str,
    target_path: &str,
) -> Result<Mount, Box<dyn std::error::Error + 'static>> {
    let git_vol = RoozVolume {
        path: target_path.into(),
        sharing: RoozVolumeSharing::Exclusive {
            key: to_safe_id(url)?,
        },
        role: RoozVolumeRole::Git,
    };

    let vol_name = git_vol.safe_volume_name()?;

    ensure_volume(docker, &vol_name, &git_vol.role.as_str()).await;

    let git_vol_mount = Mount {
        typ: Some(VOLUME),
        source: Some(vol_name.clone()),
        target: Some(git_vol.path.into()),
        read_only: Some(false),
        ..Default::default()
    };

    ensure_vol_access(docker, uid, git_vol_mount.clone(), RoozVolumeRole::Git).await?;
    Ok(git_vol_mount)
}

async fn clone_repo(
    docker: &Docker,
    image: &str,
    image_id: &str,
    user: &str,
    uid: &str,
    git_ssh_url: Option<String>,
) -> Result<(Option<RoozCfg>, Option<String>), Box<dyn std::error::Error + 'static>> {
    if let Some(url) = git_ssh_url.clone() {
        let working_dir = "/tmp/git";
        let clone_dir = format!("{}", &working_dir);

        let clone_cmd = inject(
            format!(
                    r#"export GIT_SSH_COMMAND="ssh -i /tmp/.ssh/id_ed25519 -o UserKnownHostsFile=/tmp/.ssh/known_hosts"
                    git -C {} pull || git clone {} {}"#,
                &clone_dir, &url, &clone_dir
            )
            .as_ref(),
            "clone.sh",
        );

        let git_vol_mount = git_volume(docker, uid, &url, working_dir).await?;

        let container_result = run(
            "git-clone",
            &docker,
            &image,
            &image_id,
            Some(&uid),
            None,
            &random_suffix("rooz-git"),
            Some(vec![
                git_vol_mount.clone(),
                Mount {
                    typ: Some(VOLUME),
                    source: Some(ROOZ_SSH_KEY_VOLUME_NAME.into()),
                    target: Some("/tmp/.ssh".into()),
                    read_only: Some(true),
                    ..Default::default()
                },
            ]),
            Some(vec!["cat"]),
        )
        .await?;

        let container_id = container_result.id();

        if let ContainerResult::Created { .. } = container_result {
            ensure_user(docker, container_id, user, uid).await?;

            exec_tty(
                "git-clone",
                &docker,
                &container_id,
                false,
                None,
                None,
                Some(clone_cmd.iter().map(String::as_str).collect()),
            )
            .await?;
        };

        let rooz_cfg = exec_output(
            "rooz-toml",
            &docker,
            &container_id,
            None,
            Some(vec![
                "cat",
                format!("{}/{}", clone_dir, ".rooz.toml").as_ref(),
            ]),
        )
        .await?;

        log::debug!("Repo config result: {}", &rooz_cfg);

        force_remove(docker, &container_id).await?;

        match RoozCfg::deserialize(toml::de::Deserializer::new(&rooz_cfg)).ok() {
            Some(cfg) => Ok((Some(cfg), Some(url))),
            None => Ok((None, Some(url))),
        }
    } else {
        Ok((None, None))
    }
}

fn to_safe_id(dirty: &str) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let re = Regex::new(r"[^a-zA-Z0-9_.-]").unwrap();
    Ok(re.replace_all(&dirty, "-").to_string())
}

const ROOZ_SSH_KEY_VOLUME_NAME: &'static str = "rooz-ssh-key-vol";
const VOLUME_ACCESS_IMAGE: &'static str = "alpine:latest";

async fn init_ssh_key(
    docker: &Docker,
    image: &str,
    uid: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let init_ssh = format!(
        r#"echo "Rooz init"
echo "Running in: $(pwd)"
mkdir -p /tmp/.ssh
ssh-keyscan -t ed25519 github.com 140.82.121.4 140.82.121.3 ::ffff:140.82.121.4 ::ffff:140.82.121.3 >> /tmp/.ssh/known_hosts
KEYFILE=/tmp/.ssh/id_ed25519
ls "$KEYFILE.pub" || ssh-keygen -t ed25519 -N '' -f $KEYFILE -C rooz-access-key
cat "$KEYFILE.pub"
chown -R {} /tmp/.ssh
"#,
        &uid
    );

    let init_entrypoint = inject(&init_ssh, "entrypoint.sh");

    let result = run(
        "init-ssh",
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

async fn ensure_user(
    docker: &Docker,
    container_id: &str,
    user: &str,
    uid: &str,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let ensure_user = format!(
        r#"NEWUID={}
NEWUSER={}
USER_NAME=$(id -u $NEWUID > /dev/null 2>&1 && echo -n $(id -u $NEWUID -n))

echo "Expected user: '$NEWUSER ($NEWUID)'"

[ "${{NEWUSER}}" = "${{USER_NAME}}" ] && echo "OK. Already exists." && exit

if [ -n "${{USER_NAME}}" ]
then
    echo "Another user with uid: '$NEWUID' exists: '$USER_NAME'. Renaming to '$NEWUSER'"
    usermod --login $NEWUSER $USER_NAME
else
    echo "User with uid: '$NEWUID' not found. Creating as '$NEWUSER'"

    (adduser $NEWUSER --uid $NEWUID --no-create-home --disabled-password -gecos "" || \
     useradd $NEWUSER --uid $NEWUID --no-create-home --no-log-init)
fi

mkdir /home/$NEWUSER && chown $NEWUID /home/$NEWUSER
"#,
        uid, user
    );

    let init_entrypoint = inject(&ensure_user, "entrypoint.sh");
    exec_tty(
        "ensureuser",
        docker,
        container_id,
        false,
        None,
        Some("root"),
        Some(init_entrypoint.iter().map(String::as_str).collect()),
    )
    .await?;
    Ok(())
}

async fn ensure_vol_access(
    docker: &Docker,
    uid: &str,
    mount: Mount,
    role: RoozVolumeRole,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let target_dir = mount.clone().target.unwrap();

    let dummy_file = if let RoozVolumeRole::Work = role {
        format!(
            r#"[ -z "$(ls -A {} 2>/dev/null)" ] && touch .rooz"#,
            &target_dir
        )
    } else {
        "".into()
    };

    let cmd = format!(
        r#"{}
chown {} {}"#,
        dummy_file, uid, target_dir
    );
    let entrypoint = inject(&cmd, "entrypoint.sh");

    let result = run(
        "vol-access",
        &docker,
        VOLUME_ACCESS_IMAGE,
        "ignore",
        Some("root"),
        Some(&target_dir),
        &random_suffix("rooz-vol-access"),
        Some(vec![mount.clone()]),
        Some(entrypoint.iter().map(String::as_str).collect()),
    )
    .await?;

    log::debug!(
        "[vol-access] {} ({}:{})",
        &cmd,
        mount.clone().source.unwrap(),
        role.as_str()
    );

    container_logs_to_stdout(docker, result.id()).await?;

    Ok(())
}

async fn ensure_mounts(
    docker: &Docker,
    uid: &str,
    volumes: Vec<RoozVolume>,
    is_ephemeral: bool,
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
    ];

    if is_ephemeral {
        return Ok(mounts.clone());
    }

    for v in volumes {
        log::debug!("Process volume: {:?}", &v);
        let vol_name = v.safe_volume_name()?;

        ensure_volume(&docker, &vol_name, v.role.as_str()).await;

        let mount = Mount {
            typ: Some(VOLUME),
            source: Some(vol_name.into()),
            target: Some(v.path.replace("~", &home_dir)),
            read_only: Some(false),
            ..Default::default()
        };

        ensure_vol_access(docker, uid, mount.clone(), v.role).await?;

        mounts.push(mount);
    }

    Ok(mounts.clone())
}

async fn work(
    docker: &Docker,
    image: &str,
    image_id: &str,
    shell: &str,
    uid: &str,
    user: &str,
    container_working_dir: &str,
    container_name: &str,
    is_ephemeral: bool,
    git_vol_mount: Option<Mount>,
    caches: Option<Vec<String>>,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let home_dir = format!("/home/{}", &user);
    let work_dir = format!("{}/work", &home_dir);

    let mut volumes = vec![
        RoozVolume {
            path: home_dir.clone(),
            sharing: RoozVolumeSharing::Exclusive {
                key: container_name.into(),
            },
            role: RoozVolumeRole::Home,
        },
        RoozVolume {
            path: work_dir.clone(),
            sharing: RoozVolumeSharing::Exclusive {
                key: container_name.into(),
            },
            role: RoozVolumeRole::Work,
        },
    ];

    if let Some(caches) = &caches {
        let cache_vols = caches
            .iter()
            .map(|p| RoozVolume {
                path: p.into(),
                sharing: RoozVolumeSharing::Shared,
                role: RoozVolumeRole::Cache,
            })
            .collect::<Vec<_>>();

        volumes.extend_from_slice(cache_vols.clone().as_slice());
    };

    let mut mounts = ensure_mounts(&docker, &uid, volumes, is_ephemeral, &home_dir).await?;

    if let Some(m) = git_vol_mount {
        mounts.push(m.clone());
    }

    let r = run(
        "work",
        &docker,
        &image,
        &image_id,
        Some(&uid),
        Some(&container_working_dir),
        &container_name,
        Some(mounts),
        Some(vec!["cat"]),
    )
    .await?;

    let work_id = &r.id();

    ensure_user(&docker, &work_id, &user, &uid).await?;

    exec_tty(
        "work",
        &docker,
        &work_id,
        true,
        Some(&container_working_dir),
        None,
        Some(vec![&shell]),
    )
    .await?;
    force_remove(&docker, &work_id).await?;
    Ok(())
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
            user,
            //work_dir,
            prune,
        } => {
            let ephemeral = false; // ephemeral containers won't be supported at the moment
            if prune {
                let ls_container_options = ListContainersOptions {
                    all: true,
                    filters: HashMap::from([("label", vec!["dev.rooz"])]),
                    ..Default::default()
                };
                for cs in docker.list_containers(Some(ls_container_options)).await? {
                    if let ContainerSummary { id: Some(id), .. } = cs {
                        log::debug!("Force remove container: {}", &id);
                        force_remove(&docker, &id).await?
                    }
                }

                let ls_vol_options = ListVolumesOptions {
                    filters: HashMap::from([("label", vec!["dev.rooz"])]),
                    ..Default::default()
                };

                if let Some(volumes) = docker.list_volumes(Some(ls_vol_options)).await?.volumes {
                    let rm_vol_options = RemoveVolumeOptions {
                        force: true,
                        ..Default::default()
                    };

                    for v in volumes {
                        match v {
                            Volume { ref name, .. } if name == ROOZ_SSH_KEY_VOLUME_NAME => {
                                continue;
                            }
                            _ => {}
                        };

                        log::debug!("Force remove volume: {}", &v.name);
                        docker.remove_volume(&v.name, Some(rm_vol_options)).await?
                    }
                }
            }

            let orig_shell = shell;
            let orig_user = user;
            let orig_uid = "1000".to_string();
            let orig_image = image;

            let (orig_image_id, _) = ensure_image(&docker, &orig_image).await?;

            let container_name = match &git_ssh_url {
                Some(url) => to_safe_id(&url)?,
                None => "generic".to_string(),
            };

            let ssh_key_vol_result =
                ensure_volume(&docker, ROOZ_SSH_KEY_VOLUME_NAME.into(), "ssh-key").await;

            if let VolumeResult::Created { .. } = ssh_key_vol_result {
                init_ssh_key(&docker, &orig_image_id, &orig_uid).await?;
            };

            let home_dir = format!("/home/{}", &orig_user);
            let work_dir = format!("{}/work", &home_dir);

            match clone_repo(
                &docker,
                &orig_image,
                &orig_image_id,
                &orig_user,
                &orig_uid,
                git_ssh_url.clone(),
            )
            .await?
            {
                (
                    Some(RoozCfg {
                        image: Some(img),
                        shell,
                        caches,
                        ..
                    }),
                    Some(url),
                ) => {
                    log::debug!("Image config read from .rooz.toml in the cloned repo");
                    let (image_id, _) = ensure_image(&docker, &img).await?;

                    let clone_dir = get_clone_dir(&work_dir, git_ssh_url.clone());
                    let git_vol_mount = git_volume(&docker, &orig_uid, &url, &clone_dir).await?;
                    let sh = shell.or(Some(orig_shell)).unwrap();

                    work(
                        &docker,
                        &img,
                        &image_id,
                        &sh,
                        &orig_uid,
                        &orig_user,
                        &clone_dir,
                        &container_name,
                        ephemeral,
                        Some(git_vol_mount),
                        caches,
                    )
                    .await?
                }
                (None, Some(url)) => {
                    let clone_dir = get_clone_dir(&work_dir, git_ssh_url.clone());
                    let git_vol_mount = git_volume(&docker, &orig_uid, &url, &clone_dir).await?;
                    work(
                        &docker,
                        &orig_image,
                        &orig_image_id,
                        &orig_shell,
                        &orig_uid,
                        &orig_user,
                        &clone_dir,
                        &container_name,
                        ephemeral,
                        Some(git_vol_mount),
                        None,
                    )
                    .await?
                }

                _ => {
                    work(
                        &docker,
                        &orig_image,
                        &orig_image_id,
                        &orig_shell,
                        &orig_uid,
                        &orig_user,
                        &work_dir,
                        &container_name,
                        ephemeral,
                        None,
                        None,
                    )
                    .await?
                }
            };
        }
    };
    Ok(())
}
