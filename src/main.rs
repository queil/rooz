use base64::{engine::general_purpose, Engine as _};
use bollard::container::LogOutput::{self, Console};
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions,
};
use bollard::errors::Error::{self, DockerResponseServerError};
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::MountTypeEnum::VOLUME;
use bollard::models::{CreateImageInfo, HostConfig, Mount};
use bollard::service::{ContainerInspectResponse, ContainerSummary, ImageInspect, Volume};
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
use std::process;
use std::time::Duration;
#[cfg(not(windows))]
use termion::raw::IntoRawMode;
#[cfg(not(windows))]
use termion::{async_stdin, terminal_size};
use tokio::io::AsyncWriteExt;
use tokio::task::spawn;
use tokio::time::sleep;

const DEFAULT_IMAGE: &'static str = "docker.io/bitnami/git:latest";

fn random_suffix(prefix: &str) -> String {
    let suffix: String = thread_rng()
        .sample_iter(&Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    format!("{}-{}", prefix, suffix)
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    git_ssh_url: Option<String>,
    #[arg(short, long, default_value = DEFAULT_IMAGE, env = "ROOZ_IMAGE")]
    image: String,
    #[arg(short, long)]
    pull_image: bool,
    #[arg(short, long, default_value = "bash", env = "ROOZ_SHELL")]
    shell: String,
    #[arg(short, long, default_value = "rooz_user", env = "ROOZ_USER")]
    user: String,
    #[arg(
        short,
        long,
        env = "ROOZ_CACHES",
        use_value_delimiter = true,
        help = "Enables defining global shared caches"
    )]
    caches: Option<Vec<String>>,
    #[arg(
        long,
        help = "Prunes containers and volumes scoped to the provided git repository"
    )]
    prune: bool,
    #[arg(
        long,
        conflicts_with = "prune",
        help = "Prunes all rooz containers and volumes apart from the ssh-key vol"
    )]
    prune_all: bool,
    #[arg(short, long)]
    disable_selinux: bool,
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
    pub fn group_key(&self) -> Option<String> {
        match self {
            RoozVolume {
                sharing: RoozVolumeSharing::Exclusive { key },
                ..
            } => Some(key.to_string()),
            RoozVolume {
                role: RoozVolumeRole::Cache,
                ..
            } => Some("cache".into()),
            _ => None,
        }
    }
}

struct WorkSpec<'a> {
    image: &'a str,
    image_id: &'a str,
    shell: &'a str,
    uid: &'a str,
    user: &'a str,
    container_working_dir: &'a str,
    container_name: &'a str,
    is_ephemeral: bool,
    git_vol_mount: Option<Mount>,
    caches: Option<Vec<String>>,
    disable_selinux: bool,
}

struct RunSpec<'a> {
    reason: &'a str,
    image: &'a str,
    image_id: &'a str,
    user: Option<&'a str>,
    work_dir: Option<&'a str>,
    container_name: &'a str,
    mounts: Option<Vec<Mount>>,
    entrypoint: Option<Vec<&'a str>>,
    disable_selinux: bool,
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

async fn run<'a>(
    docker: &Docker,
    spec: RunSpec<'a>,
) -> Result<ContainerResult, Box<dyn std::error::Error + 'static>> {
    log::debug!(
        "[{}]: running {} as {:?} using image {}",
        &spec.reason,
        spec.container_name,
        spec.user,
        spec.image
    );

    let container_id = match docker.inspect_container(&spec.container_name, None).await {
        Ok(ContainerInspectResponse {
            id: Some(id),
            image: Some(img),
            ..
        }) if img.to_owned() == spec.image_id => ContainerResult::Reused { id },
        s => {
            let remove_options = RemoveContainerOptions {
                force: true,
                ..Default::default()
            };

            if let Ok(ContainerInspectResponse { id: Some(id), .. }) = s {
                docker.remove_container(&id, Some(remove_options)).await?;
            }

            let options = CreateContainerOptions {
                name: spec.container_name,
                platform: None,
            };

            let host_config = HostConfig {
                auto_remove: Some(true),
                mounts: spec.mounts,
                security_opt: if spec.disable_selinux {
                    Some(vec!["label=disable".to_string()])
                } else {
                    None
                },
                ..Default::default()
            };

            let config = Config {
                image: Some(spec.image),
                entrypoint: spec.entrypoint,
                working_dir: spec.work_dir,
                user: spec.user,
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

async fn ensure_volume(
    docker: &Docker,
    name: &str,
    role: &str,
    group_key: Option<String>,
) -> VolumeResult {
    let wsk = group_key.unwrap_or_default();
    let labels = HashMap::from([
        ("dev.rooz", "true"),
        ("dev.rooz.role", role),
        ("dev.rooz.group-key", &wsk),
    ]);

    let create_vol_options = CreateVolumeOptions::<&str> {
        name,
        labels: labels,
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

async fn pull_image(
    docker: &Docker,
    image: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + 'static>> {
    println!("Pulling image: {}", &image);
    let img_chunks = &image.split(':').collect::<Vec<&str>>();
    let mut image_info = docker.create_image(
        Some(CreateImageOptions::<&str> {
            from_image: img_chunks[0],
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
            Err(Error::DockerStreamError { error }) => eprintln!("{}", error),
            e => panic!("{:?}", e),
        };
    }
    println!("");
    Ok(docker.inspect_image(&image).await?.id)
}

async fn ensure_image(
    docker: &Docker,
    image: &str,
    pull: bool,
) -> Result<String, Box<dyn std::error::Error + 'static>> {
    let image_id = match docker.inspect_image(&image).await {
        Ok(ImageInspect { id, .. }) => {
            if pull {
                pull_image(docker, image).await?
            } else {
                id
            }
        }
        Err(DockerResponseServerError {
            status_code: 404, ..
        }) => pull_image(docker, image).await?,
        Err(e) => panic!("{:?}", e),
    };

    log::debug!("Image ID: {:?}", image_id);
    Ok(image_id.unwrap())
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

    ensure_volume(
        docker,
        &vol_name,
        &git_vol.role.as_str(),
        git_vol.group_key(),
    )
    .await;

    let git_vol_mount = Mount {
        typ: Some(VOLUME),
        source: Some(vol_name.clone()),
        target: Some(git_vol.path.into()),
        read_only: Some(false),
        ..Default::default()
    };

    Ok(git_vol_mount)
}

async fn clone_repo(
    docker: &Docker,
    image: &str,
    image_id: &str,
    uid: &str,
    git_ssh_url: Option<String>,
) -> Result<(Option<RoozCfg>, Option<String>), Box<dyn std::error::Error + 'static>> {
    if let Some(url) = git_ssh_url.clone() {
        let working_dir = "/tmp/git";
        let clone_dir = format!("{}", &working_dir);

        let clone_cmd = inject(
            format!(
                    r#"export GIT_SSH_COMMAND="ssh -i /tmp/.ssh/id_ed25519 -o UserKnownHostsFile=/tmp/.ssh/known_hosts"
                    ls "{}/.git" > /dev/null 2>&1 || git clone {} {}"#,
                &clone_dir, &url, &clone_dir
            )
            .as_ref(),
            "clone.sh",
        );

        let git_vol_mount = git_volume(docker, &url, working_dir).await?;

        let run_spec = RunSpec {
            reason: "git-clone",
            image,
            image_id,
            user: Some(&uid),
            work_dir: None,
            container_name: &random_suffix("rooz-git"),
            mounts: Some(vec![
                git_vol_mount.clone(),
                Mount {
                    typ: Some(VOLUME),
                    source: Some(ROOZ_SSH_KEY_VOLUME_NAME.into()),
                    target: Some("/tmp/.ssh".into()),
                    read_only: Some(true),
                    ..Default::default()
                },
            ]),
            entrypoint: Some(vec!["cat"]),
            disable_selinux: false,
        };

        let container_result = run(&docker, run_spec).await?;

        let container_id = container_result.id();

        if let ContainerResult::Created { .. } = container_result {
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
    Ok(re.replace_all(&dirty, "-").to_ascii_lowercase().to_string())
}

const ROOZ_SSH_KEY_VOLUME_NAME: &'static str = "rooz-ssh-key-vol";

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

    let run_spec = RunSpec {
        reason: "init-ssh",
        image,
        image_id: "ignore",
        user: Some("root"),
        work_dir: None,
        container_name: "rooz-init-ssh",
        mounts: Some(vec![Mount {
            typ: Some(VOLUME),
            read_only: Some(false),
            source: Some(ROOZ_SSH_KEY_VOLUME_NAME.into()),
            target: Some("/tmp/.ssh".into()),
            ..Default::default()
        }]),
        entrypoint: Some(init_entrypoint.iter().map(String::as_str).collect()),
        disable_selinux: false,
    };

    let result = run(&docker, run_spec).await?;

    container_logs_to_stdout(docker, result.id()).await?;

    Ok(())
}

async fn ensure_mounts(
    docker: &Docker,
    volumes: Vec<RoozVolume>,
    is_ephemeral: bool,
    home_dir: &str,
) -> Result<Vec<Mount>, Box<dyn std::error::Error + 'static>> {
    let mut mounts = vec![Mount {
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
    }];

    if is_ephemeral {
        return Ok(mounts.clone());
    }

    for v in volumes {
        log::debug!("Process volume: {:?}", &v);
        let vol_name = v.safe_volume_name()?;

        ensure_volume(&docker, &vol_name, v.role.as_str(), v.group_key()).await;

        let mount = Mount {
            typ: Some(VOLUME),
            source: Some(vol_name.into()),
            target: Some(v.path.replace("~", &home_dir)),
            read_only: Some(false),
            ..Default::default()
        };

        mounts.push(mount);
    }

    Ok(mounts.clone())
}

async fn work<'a>(
    docker: &Docker,
    spec: WorkSpec<'a>,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let home_dir = format!("/home/{}", &spec.user);
    let work_dir = format!("{}/work", &home_dir);

    let mut volumes = vec![
        RoozVolume {
            path: home_dir.clone(),
            sharing: RoozVolumeSharing::Exclusive {
                key: spec.container_name.into(),
            },
            role: RoozVolumeRole::Home,
        },
        RoozVolume {
            path: work_dir.clone(),
            sharing: RoozVolumeSharing::Exclusive {
                key: spec.container_name.into(),
            },
            role: RoozVolumeRole::Work,
        },
    ];

    if let Some(caches) = &spec.caches {
        log::debug!("Processing caches");
        let cache_vols = caches
            .iter()
            .map(|p| RoozVolume {
                path: p.to_string(),
                sharing: RoozVolumeSharing::Shared,
                role: RoozVolumeRole::Cache,
            })
            .collect::<Vec<_>>();

        for c in caches {
            log::debug!("Cache: {}", c);
        }

        volumes.extend_from_slice(cache_vols.clone().as_slice());
    } else {
        log::debug!("No caches configured. Skipping");
    }

    let mut mounts = ensure_mounts(&docker, volumes, spec.is_ephemeral, &home_dir).await?;

    if let Some(m) = spec.git_vol_mount {
        mounts.push(m.clone());
    }

    let run_spec = RunSpec {
        reason: "work",
        image: &spec.image,
        image_id: &spec.image_id,
        user: Some(&spec.uid),
        work_dir: Some(&spec.container_working_dir),
        container_name: &spec.container_name,
        mounts: Some(mounts),
        entrypoint: Some(vec!["cat"]),
        disable_selinux: spec.disable_selinux,
    };

    let r = run(&docker, run_spec).await?;

    let work_id = &r.id();

    exec_tty(
        "work",
        &docker,
        &work_id,
        true,
        Some(&spec.container_working_dir),
        None,
        Some(vec![&spec.shell]),
    )
    .await?;
    force_remove(&docker, &work_id).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + 'static>> {
    env_logger::init();

    log::debug!("Started");

    let args = Cli::parse();
    let docker = Docker::connect_with_local_defaults().expect("Docker API connection established");

    log::debug!("API connected");

    match args {
        Cli {
            git_ssh_url,
            image,
            pull_image,
            shell,
            user,
            //work_dir,
            prune,
            prune_all,
            disable_selinux,
            caches,
        } => {
            let ephemeral = false; // ephemeral containers won't be supported at the moment

            let container_name = match &git_ssh_url {
                Some(url) => to_safe_id(&url)?,
                None => "rooz-generic".to_string(),
            };

            if prune || prune_all {
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

                let group_key_filter = format!("dev.rooz.group-key={}", &container_name);
                let mut filters = HashMap::from([("label", vec!["dev.rooz"])]);
                if !prune_all {
                    filters.insert("label", vec![&group_key_filter]);
                }
                let ls_vol_options = ListVolumesOptions {
                    filters,
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
                log::debug!("Prune success");
                process::exit(0);
            }

            let orig_shell = shell;
            let orig_user = user;
            let orig_uid = "1000".to_string();
            let orig_image = image;

            let orig_image_id = ensure_image(&docker, &orig_image, pull_image).await?;

            let ssh_key_vol_result = ensure_volume(
                &docker,
                ROOZ_SSH_KEY_VOLUME_NAME.into(),
                "ssh-key",
                Some("ssh-key".into()),
            )
            .await;

            if let VolumeResult::Created { .. } = ssh_key_vol_result {
                init_ssh_key(&docker, &orig_image_id, &orig_uid).await?;
            };

            let home_dir = format!("/home/{}", &orig_user);
            let work_dir = format!("{}/work", &home_dir);

            let work_spec = WorkSpec {
                image: &orig_image,
                image_id: &orig_image_id,
                shell: &orig_shell,
                uid: &orig_uid,
                user: &orig_user,
                container_working_dir: &work_dir,
                container_name: &container_name,
                is_ephemeral: ephemeral,
                git_vol_mount: None,
                caches: caches.clone(),
                disable_selinux,
            };

            match clone_repo(
                &docker,
                &orig_image,
                &orig_image_id,
                &orig_uid,
                git_ssh_url.clone(),
            )
            .await?
            {
                (
                    Some(RoozCfg {
                        image: Some(img),
                        shell,
                        caches: repo_caches,
                        ..
                    }),
                    Some(url),
                ) => {
                    log::debug!("Image config read from .rooz.toml in the cloned repo");
                    let image_id = ensure_image(&docker, &img, pull_image).await?;
                    let clone_dir = get_clone_dir(&work_dir, git_ssh_url.clone());
                    let git_vol_mount = git_volume(&docker, &url, &clone_dir).await?;
                    let sh = shell.or(Some(orig_shell.to_string())).unwrap();
                    let mut all_caches = vec![];
                    if let Some(caches) = caches {
                        all_caches.extend(caches);
                    }
                    if let Some(caches) = repo_caches {
                        all_caches.extend(caches);
                    };

                    all_caches.dedup();

                    work(
                        &docker,
                        WorkSpec {
                            image: &img,
                            image_id: &image_id,
                            shell: &sh,
                            container_working_dir: &clone_dir,
                            git_vol_mount: Some(git_vol_mount),
                            caches: Some(all_caches),
                            ..work_spec
                        },
                    )
                    .await?
                }
                (None, Some(url)) => {
                    let clone_dir = get_clone_dir(&work_dir, git_ssh_url.clone());
                    let git_vol_mount = git_volume(&docker, &url, &clone_dir).await?;
                    work(
                        &docker,
                        WorkSpec {
                            container_working_dir: &clone_dir,
                            git_vol_mount: Some(git_vol_mount),
                            ..work_spec
                        },
                    )
                    .await?
                }

                _ => work(&docker, work_spec).await?,
            };
        }
    };
    Ok(())
}
