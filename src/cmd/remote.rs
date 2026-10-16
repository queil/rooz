use bollard::{API_DEFAULT_VERSION, Docker, query_parameters::ListContainersOptions};

use bollard_stubs::models::PortSummary;
use openssh::{ForwardType, KnownHosts, Session, SessionBuilder};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    net::{Ipv4Addr, TcpListener},
    os::unix::{fs::PermissionsExt, io::AsRawFd},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};
use tabled::{Table, Tabled, settings::Style};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::watch,
    task::JoinSet,
};

use crate::{
    cli::{RemoteAddParams, RemoteCommands, RemoteParams, RemoteRmParams},
    config::remotes::{self, Host, Remotes},
    model::types::AnyError,
    util::{
        labels::{self, Labels},
        progress,
    },
};

const LOCALHOST_IP: &str = "127.0.0.1";
const POLL_INTERVAL: Duration = Duration::from_secs(10);
const MIN_BACKOFF: Duration = Duration::from_secs(3);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

// local port -> name of the host whose tunnel currently owns it
type Claims = Arc<Mutex<HashMap<u16, String>>>;

#[derive(Debug, Clone)]
pub struct Tunnel {
    pub local_port: u16,
    pub container_name: String,
    pub is_active: bool,
}

struct SocketLock(#[allow(dead_code)] File);

// Held for as long as a host is served. It makes removing a leftover socket file safe:
// whoever holds the lock is the only process allowed to own that socket.
fn lock_socket(name: &str, socket_path: &Path) -> Result<SocketLock, AnyError> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(socket_path.with_extension("lock"))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!(
            "another 'rooz remote' is already serving host '{}' ({})",
            name,
            socket_path.display()
        )
        .into());
    }
    Ok(SocketLock(file))
}

async fn connect(
    builder: &SessionBuilder,
    name: &str,
    host: &Host,
    local_socket_path: &Path,
) -> Result<Session, AnyError> {
    if local_socket_path.exists() {
        fs::remove_file(local_socket_path)?;
    }

    let session = builder.connect_mux(&host.ssh).await?;

    println!(
        "{} SSH: connected to {}",
        progress::host_tag(&name),
        &host.ssh
    );

    let socket_url = match &host.socket {
        Some(socket) => socket.clone(),
        None => String::from_utf8(
            session
                .command("echo")
                .arg("-n")
                .raw_arg("$DOCKER_HOST")
                .output()
                .await?
                .stdout,
        )?,
    };

    if socket_url.is_empty() {
        return Err(format!(
            "env var DOCKER_HOST is not set on {}. Set it there or give the host a 'socket'",
            &host.ssh
        )
        .into());
    }

    log::debug!("[{}] Remote socket: {}", name, socket_url);

    let remote_socket_path = Path::new(&socket_url);
    let remote_socket = remote_socket_path
        .strip_prefix("unix://")
        .unwrap_or(&remote_socket_path);

    session
        .request_port_forward(ForwardType::Local, local_socket_path, remote_socket)
        .await?;

    println!(
        "{} Forwarding: {} -> {}:{}",
        progress::host_tag(&name),
        local_socket_path.display(),
        &host.ssh,
        &remote_socket.display()
    );
    println!(
        "{} Use it with 'rooz -H {}' (or ROOZ_HOST={}); for other tools: DOCKER_HOST=unix://{}",
        progress::host_tag(&name),
        name,
        name,
        local_socket_path.display()
    );
    Ok(session)
}

async fn test_tunnel(host: &str, port: u16) -> Result<bool, AnyError> {
    log::debug!("Testing tunnel: {}:{}", host, port);

    let mut stream = match TcpStream::connect((host, port)).await {
        Ok(s) => s,
        Err(e) => {
            log::debug!("Could not connect: {}", e);
            return Ok(false);
        }
    };

    let mut response = vec![0; 256];
    match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut response)).await {
        Ok(Ok(n)) if n > 0 => {
            let response_str = String::from_utf8_lossy(&response[..n]);
            if response_str.starts_with("SSH-") {
                log::debug!("Tunnel: OK (SSH)");
                return Ok(true);
            }
        }
        _ => {}
    }

    let request = "HEAD / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    stream.write_all(request.as_bytes()).await?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;

    let response_str = String::from_utf8_lossy(&response);
    let is_success = response_str.starts_with("HTTP/");
    log::debug!("Tunnel: {}", if is_success { "OK" } else { "Dead" });
    Ok(is_success)
}

async fn open_tunnel(session: &Session, local_port: u16, remote_port: u16) -> Result<(), AnyError> {
    Ok(session
        .request_port_forward(
            ForwardType::Local,
            (Ipv4Addr::new(127, 0, 0, 1), local_port),
            (Ipv4Addr::new(127, 0, 0, 1), remote_port),
        )
        .await?)
}

async fn close_port_forward(
    session: &Session,
    local_port: u16,
    remote_port: u16,
) -> Result<(), AnyError> {
    Ok(session
        .close_port_forward(
            ForwardType::Local,
            (Ipv4Addr::new(127, 0, 0, 1), local_port),
            (Ipv4Addr::new(127, 0, 0, 1), remote_port),
        )
        .await
        .unwrap_or_else(|e| log::debug!("Failed closing tunnel: {}", e)))
}

fn release(claims: &Claims, host: &str, local_port: u16) {
    let mut claims = claims.lock().unwrap();
    if claims.get(&local_port).is_some_and(|owner| owner == host) {
        claims.remove(&local_port);
    }
}

async fn close_tunnel(
    host: &str,
    session: &Session,
    claims: &Claims,
    remote_port: &u16,
    open_tunnels_map: &mut HashMap<u16, Tunnel>,
) -> Result<(), AnyError> {
    let Tunnel {
        local_port,
        container_name,
        ..
    } = &open_tunnels_map[remote_port];
    let local_socket = format!("{}:{}", LOCALHOST_IP, local_port);
    let remote_socket = format!("{}:{}", container_name, remote_port);
    log::debug!(
        "[{}] Closing tunnel: {} -> {}",
        host,
        local_socket,
        remote_socket
    );
    close_port_forward(&session, *local_port, *remote_port).await?;
    release(claims, host, *local_port);
    open_tunnels_map.remove(remote_port);
    Ok(())
}

async fn get_pid_using_port(local_port: &str) -> Result<Option<u32>, AnyError> {
    let output = Command::new("lsof")
        .args(["-ti", &format!("tcp:{}", local_port)])
        .output()?;

    if !output.stdout.is_empty() {
        let pid = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(pid.trim().parse::<u32>().ok())
    } else {
        Ok(None)
    }
}

fn is_available(port: &u16) -> bool {
    TcpListener::bind(("127.0.0.1", *port)).ok().is_some()
}

async fn get_docker_ports(docker: &Docker) -> Result<HashMap<u16, Tunnel>, AnyError> {
    let containers = match docker
        .list_containers(Some(ListContainersOptions {
            filters: Some(Labels::default().into()),
            ..Default::default()
        }))
        .await
    {
        Ok(data) => data,
        Err(e) => {
            log::debug!("Failed reading ports from Docker API: {}", e);
            vec![]
        }
    };

    Ok(containers
        .iter()
        .flat_map(|c| {
            let names = c
                .names
                .as_ref()
                .map(|n| n.concat())
                .unwrap_or(c.id.as_ref().unwrap().to_string());
            let ports = c.clone().ports.unwrap_or(Vec::<_>::new());

            let forward_ports_str = c
                .labels
                .as_ref()
                .and_then(|l| l.get(labels::FORWARD_PORTS))
                .map(|s| s.as_str())
                .unwrap_or("");

            let forward_ports: HashSet<u16> = forward_ports_str
                .split(',')
                .filter_map(|p| p.trim().parse::<u16>().ok())
                .collect();

            ports
                .iter()
                .filter(|PortSummary { private_port, .. }| forward_ports.contains(private_port))
                .map(
                    |PortSummary {
                         private_port,
                         public_port,
                         ..
                     }| {
                        (
                            public_port.unwrap_or(*private_port),
                            Tunnel {
                                local_port: public_port.unwrap_or(*private_port),
                                container_name: names.to_string(),
                                is_active: false,
                            },
                        )
                    },
                )
                .collect::<Vec<_>>()
        })
        .collect::<HashMap<u16, Tunnel>>())
}

async fn manage_tunnels(
    host: &str,
    docker: &Docker,
    session: &Session,
    claims: &Claims,
    skipped: &mut HashSet<u16>,
    open_tunnels_map: &mut HashMap<u16, Tunnel>,
) -> Result<(), AnyError> {
    session.check().await?;

    let docker_ports_map = get_docker_ports(&docker).await?;

    let docker_ports: HashSet<_> = docker_ports_map.keys().cloned().collect();
    let open_tunnels: HashSet<_> = open_tunnels_map.keys().cloned().collect();

    skipped.retain(|p| docker_ports.contains(p));

    let new_ports = docker_ports.difference(&open_tunnels);
    let stale_ports = open_tunnels.difference(&docker_ports);
    let current_ports = docker_ports.intersection(&open_tunnels);

    for remote_port in current_ports {
        let tunnel = &open_tunnels_map[remote_port];
        let Tunnel {
            local_port,
            container_name,
            is_active,
        } = tunnel;
        let local_socket = format!("{}:{}", LOCALHOST_IP, local_port);
        let remote_socket = format!("{}:{}", container_name, remote_port);
        let was_active = is_active;
        let now_active = test_tunnel(LOCALHOST_IP, *local_port).await?;

        match (was_active, now_active) {
            (true, false) => {
                eprintln!(
                    "{} Tunnel endpoint is now down: {} -> {}",
                    progress::host_tag(&host),
                    local_socket,
                    remote_socket
                );
                open_tunnels_map.insert(
                    *remote_port,
                    Tunnel {
                        is_active: false,
                        ..tunnel.clone()
                    },
                );
            }
            (false, true) => {
                eprintln!(
                    "{} Tunnel endpoint is now up: {} -> {}",
                    progress::host_tag(&host),
                    local_socket,
                    remote_socket
                );
                open_tunnels_map.insert(
                    *remote_port,
                    Tunnel {
                        is_active: true,
                        ..tunnel.clone()
                    },
                );
            }
            (_, _) => (),
        }
    }

    for remote_port in stale_ports {
        let Tunnel {
            local_port,
            container_name,
            ..
        } = &open_tunnels_map[remote_port];
        let local_socket = format!("{}:{}", LOCALHOST_IP, local_port);
        let remote_socket = format!("{}:{}", container_name, remote_port);
        eprintln!(
            "{} Dead tunnel - closing: {} {}",
            progress::host_tag(&host),
            local_socket,
            remote_socket
        );
        close_tunnel(host, &session, claims, remote_port, open_tunnels_map).await?;
    }

    for remote_port in new_ports {
        let Tunnel {
            local_port,
            container_name,
            ..
        } = &docker_ports_map[remote_port];

        let owner = claims.lock().unwrap().get(local_port).cloned();
        if let Some(owner) = owner {
            if skipped.insert(*local_port) {
                eprintln!(
                    "{} Local port {} is already forwarded for host '{}' - skipping",
                    progress::host_tag(&host),
                    local_port,
                    owner
                );
            }
            continue;
        }

        if is_available(&local_port) {
            let local_socket = format!("{}:{}", LOCALHOST_IP, local_port);
            let remote_socket = format!("{}:{}", container_name, remote_port);
            log::debug!(
                "[{}] Opening tunnel: {} -> {}",
                host,
                local_socket,
                remote_socket
            );
            open_tunnel(&session, *local_port, *remote_port).await?;
            claims.lock().unwrap().insert(*local_port, host.to_string());
            let is_active = test_tunnel(LOCALHOST_IP, *local_port).await?;
            open_tunnels_map.insert(
                *remote_port,
                Tunnel {
                    local_port: *local_port,
                    container_name: container_name.to_string(),
                    is_active,
                },
            );
            eprintln!(
                "{} Opened tunnel (endpoint: {}): {} -> {}",
                progress::host_tag(&host),
                if is_active { "UP" } else { "DOWN" },
                local_socket,
                remote_socket
            );
        } else if skipped.insert(*local_port) {
            let binding_pid = get_pid_using_port(&local_port.to_string()).await?;
            eprintln!(
                "{} Local port {} is already bound by another process: {:?} - skipping",
                progress::host_tag(&host),
                local_port,
                binding_pid
            );
        }
    }
    Ok(())
}

async fn pause(stop: &mut watch::Receiver<bool>, duration: Duration) {
    tokio::select! {
        _ = tokio::time::sleep(duration) => {}
        _ = stop.changed() => {}
    }
}

fn drop_tunnels(host: &str, claims: &Claims, tunnels: &mut HashMap<u16, Tunnel>) {
    for (_, Tunnel { local_port, .. }) in tunnels.drain() {
        release(claims, host, local_port);
    }
}

async fn serve(
    name: String,
    host: Host,
    claims: Claims,
    mut stop: watch::Receiver<bool>,
) -> Result<(), AnyError> {
    let local_socket_path = remotes::socket_path(&name)?;
    let _lock = lock_socket(&name, &local_socket_path)?;

    let mut builder = SessionBuilder::default();
    builder
        .known_hosts_check(KnownHosts::Strict)
        .connect_timeout(Duration::from_secs(5))
        .server_alive_interval(Duration::from_secs(5));

    let mut session: Option<Session> = None;
    let mut tunnels = HashMap::<u16, Tunnel>::new();
    let mut skipped = HashSet::<u16>::new();
    let mut backoff = MIN_BACKOFF;

    while !*stop.borrow() {
        if let Some(s) = &session {
            if s.check().await.is_err() {
                eprintln!(
                    "{} SSH connection lost. Reconnecting...",
                    progress::host_tag(&name)
                );
                // forwards die with the session
                drop_tunnels(&name, &claims, &mut tunnels);
                skipped.clear();
                if let Some(s) = session.take() {
                    let _ = s.close().await;
                }
            }
        }

        let Some(s) = &session else {
            match connect(&builder, &name, &host, &local_socket_path).await {
                Ok(s) => {
                    session = Some(s);
                    backoff = MIN_BACKOFF;
                }
                Err(error) => {
                    eprintln!("{} ERROR: {}", progress::host_tag(&name), error);
                    pause(&mut stop, backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
            continue;
        };

        match Docker::connect_with_unix(
            &local_socket_path.to_string_lossy(),
            10,
            API_DEFAULT_VERSION,
        ) {
            Ok(docker) => {
                let docker = docker.with_timeout(Duration::from_secs(10));
                if let Err(e) =
                    manage_tunnels(&name, &docker, s, &claims, &mut skipped, &mut tunnels).await
                {
                    log::debug!("[{}] Connection failed. Will retry: {}", name, e);
                }
            }
            Err(e) => log::debug!("[{}] Failed connect to Docker API. Will retry: {}", name, e),
        }
        pause(&mut stop, POLL_INTERVAL).await;
    }

    if let Some(s) = session {
        for (
            remote_port,
            Tunnel {
                local_port,
                container_name,
                ..
            },
        ) in tunnels.drain()
        {
            close_port_forward(&s, local_port, remote_port).await?;
            release(&claims, &name, local_port);
            println!(
                "{} Closing: {}:{} -> {}:{}",
                progress::host_tag(&name),
                LOCALHOST_IP,
                local_port,
                container_name,
                remote_port
            );
        }
        s.close().await?;
    }
    let _ = fs::remove_file(&local_socket_path);
    Ok(())
}

pub async fn remote(hosts: Vec<(String, Host)>) -> Result<(), AnyError> {
    let socket_dir = remotes::socket_dir()?;
    fs::create_dir_all(&socket_dir)?;
    // a forwarded engine socket is as good as a root shell on the host: keep it ours
    fs::set_permissions(&socket_dir, fs::Permissions::from_mode(0o700))?;

    let (stop_tx, stop_rx) = watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = stop_tx.send(true);
    });

    let claims = Claims::default();
    let mut tasks = JoinSet::new();
    for (name, host) in hosts {
        let (claims, stop) = (claims.clone(), stop_rx.clone());
        tasks.spawn(async move {
            let result = serve(name.clone(), host, claims, stop).await;
            (name, result)
        });
    }

    let mut failed = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((_, Ok(()))) => (),
            Ok((name, Err(e))) => {
                eprintln!("{} ERROR: {}", progress::host_tag(&name), e);
                failed.push(name);
            }
            Err(e) => return Err(e.into()),
        }
    }
    match failed.is_empty() {
        true => Ok(()),
        false => Err(format!("failed hosts: {}", failed.join(", ")).into()),
    }
}

#[derive(Tabled)]
struct HostView {
    #[tabled(rename = "HOST")]
    name: String,
    #[tabled(rename = "SSH")]
    ssh: String,
    #[tabled(rename = "SOCKET")]
    socket: String,
    #[tabled(rename = "CONNECTED", format("{}", if self.connected {"true"} else {""}))]
    connected: bool,
}

pub async fn run(params: &RemoteParams) -> Result<(), AnyError> {
    match &params.command {
        None => {
            let hosts = Remotes::load()?.select(&params.names)?;
            remote(hosts).await
        }
        Some(RemoteCommands::Add(RemoteAddParams {
            name,
            ssh_url,
            socket,
        })) => {
            remotes::validate_name(name)?;
            let mut remotes = Remotes::load()?;
            let replaced = remotes
                .hosts
                .insert(
                    name.clone(),
                    Host {
                        ssh: ssh_url.clone(),
                        socket: socket.clone(),
                    },
                )
                .is_some();
            let path = remotes.save()?;
            println!(
                "{} host '{}' in {}",
                if replaced { "Updated" } else { "Added" },
                name,
                path.display()
            );
            Ok(())
        }
        Some(RemoteCommands::Rm(RemoteRmParams { name })) => {
            let mut remotes = Remotes::load()?;
            remotes.get(name)?;
            remotes.hosts.remove(name);
            let path = remotes.save()?;
            println!("Removed host '{}' from {}", name, path.display());
            Ok(())
        }
        Some(RemoteCommands::Ls) => {
            let views = Remotes::load()?
                .hosts
                .into_iter()
                .map(|(name, host)| {
                    Ok(HostView {
                        connected: remotes::is_connected(&remotes::socket_path(&name)?),
                        name,
                        ssh: host.ssh,
                        socket: host.socket.unwrap_or_default(),
                    })
                })
                .collect::<Result<Vec<_>, AnyError>>()?;
            println!("{}", Table::new(views).with(Style::blank()));
            Ok(())
        }
    }
}

// Each host's workspaces are listed by a child process pointed at that host's socket
pub fn list_all_hosts() -> Result<(), AnyError> {
    let exe = std::env::current_exe()?;
    for name in Remotes::load()?.hosts.keys() {
        println!("== {} ==", progress::host(name));
        if !remotes::is_connected(&remotes::socket_path(name)?) {
            println!("not connected (run 'rooz remote {}')\n", name);
            continue;
        }
        let status = std::process::Command::new(&exe)
            .args(["-H", name, "list"])
            .status()?;
        if !status.success() {
            eprintln!("'{}': list failed ({})", name, status);
        }
        println!();
    }
    Ok(())
}

// Names the engine a destructive command is about to hit
pub fn announce_target(host: Option<&str>) {
    let target = match (
        host,
        std::env::var("DOCKER_HOST").ok().filter(|s| !s.is_empty()),
    ) {
        (Some(name), _) => format!("host '{}'", name),
        (None, Some(docker_host)) => format!("DOCKER_HOST {}", docker_host),
        (None, None) => "the local engine".to_string(),
    };
    eprintln!("Target: {}", target);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_on_same_socket_is_refused() {
        let dir = std::env::temp_dir().join(format!("rooz-lock-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("a.sock");
        let first = lock_socket("a", &socket).unwrap();
        assert!(lock_socket("a", &socket).is_err());
        assert!(lock_socket("b", &dir.join("b.sock")).is_ok());
        drop(first);
        assert!(lock_socket("a", &socket).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_only_drops_own_claims() {
        let claims = Claims::default();
        claims.lock().unwrap().insert(8080, "a".into());
        release(&claims, "b", 8080);
        assert!(claims.lock().unwrap().contains_key(&8080));
        release(&claims, "a", 8080);
        assert!(claims.lock().unwrap().is_empty());
    }
}
