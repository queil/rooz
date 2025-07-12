use bollard::{models::Port, query_parameters::ListContainersOptions, Docker};

use openssh::{ForwardType, KnownHosts, Session, SessionBuilder};
use regex::Regex;
use std::{
    collections::{HashMap, HashSet},
    fs,
    net::{Ipv4Addr, TcpListener},
    path::Path,
    process::Command,
    sync::{
        mpsc::{self, Sender},
        Mutex,
    },
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use crate::{model::types::AnyError, util::labels};

const LOCALHOST_IP: &str = "127.0.0.1";

#[derive(Debug, Clone)]
pub struct Tunnel {
    pub local_port: u16,
    pub container_name: String,
    pub is_active: bool,
}

async fn connect(
    builder: &SessionBuilder,
    ssh_url: &str,
    local_socket_path: &Path,
) -> Result<Session, AnyError> {
    if local_socket_path.exists() {
        fs::remove_file(local_socket_path)?;
    }

    let session = builder.connect_mux(&ssh_url).await?;

    println!("SSH: connected to {}", &ssh_url);

    let socket_url = String::from_utf8(
        session
            .command("echo")
            .arg("-n")
            .raw_arg("$DOCKER_HOST")
            .output()
            .await?
            .stdout,
    )?;

    if socket_url.is_empty() {
        panic!("Env var DOCKER_HOST is not set on the remote host. Can't get docker.socket path.")
    }

    log::debug!(
        "Read remote socket from env var DOCKER_HOST: {}",
        socket_url
    );

    let remote_socket_path = Path::new(&socket_url);
    let remote_socket = remote_socket_path
        .strip_prefix("unix://")
        .unwrap_or(&remote_socket_path);

    session
        .request_port_forward(ForwardType::Local, local_socket_path, remote_socket)
        .await?;

    println!(
        "Forwarding: {} -> {}:{}",
        local_socket_path.display(),
        &ssh_url,
        &remote_socket.display()
    );
    println!(
        "Run 'export DOCKER_HOST=unix://{}' to make the socket useful for local tools",
        local_socket_path.display()
    );
    Ok(session)
}

async fn test_http_tunnel(host: &str, port: u16) -> Result<bool, Box<dyn std::error::Error>> {
    log::debug!("Testing tunnel: {}:{}", host, port);

    let mut stream = match TcpStream::connect((host, port)).await {
        Ok(s) => s,
        Err(e) => {
            log::debug!("Could not connect: {}", e);
            return Ok(false);
        }
    };

    let request = "HEAD / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    stream.write_all(request.as_bytes()).await?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;

    let response_str = String::from_utf8_lossy(&response);
    let is_success = response_str.starts_with("HTTP/1.1") || response_str.starts_with("HTTP/2.0");
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

async fn close_tunnel(
    session: &Session,
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
    log::debug!("Closing tunnel: {} -> {}", local_socket, remote_socket);
    close_port_forward(&session, *local_port, *remote_port).await?;
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
            filters: Some((&labels::Labels::default()).into()),
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

            ports
                .iter()
                .map(
                    |Port {
                         private_port,
                         public_port,
                         ..
                     }| {
                        (
                            public_port.unwrap_or(*private_port),
                            Tunnel {
                                local_port: *private_port,
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

pub async fn manage_tunnels(
    docker: &Docker,
    session: &Session,
    open_tunnels_map: &mut HashMap<u16, Tunnel>,
) -> Result<(), AnyError> {
    session.check().await?;

    let docker_ports_map = get_docker_ports(&docker).await?;

    let docker_ports: HashSet<_> = docker_ports_map.keys().cloned().collect();
    let open_tunnels: HashSet<_> = open_tunnels_map.keys().cloned().collect();

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
        let now_active = test_http_tunnel(LOCALHOST_IP, *local_port).await?;

        match (was_active, now_active) {
            (true, false) => {
                eprintln!(
                    "Tunnel endpoint is now down: {} -> {}",
                    local_socket, remote_socket
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
                    "Tunnel endpoint is now up: {} -> {}",
                    local_socket, remote_socket
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
        eprintln!("Dead tunnel - closing: {} {}", local_socket, remote_socket);
        close_tunnel(&session, remote_port, open_tunnels_map).await?;
    }

    for remote_port in new_ports {
        let Tunnel {
            local_port,
            container_name,
            ..
        } = &docker_ports_map[remote_port];
        if is_available(&local_port) {
            let local_socket = format!("{}:{}", LOCALHOST_IP, local_port);
            let remote_socket = format!("{}:{}", container_name, remote_port);
            log::debug!("Opening tunnel: {} -> {}", local_socket, remote_socket);
            open_tunnel(&session, *local_port, *remote_port).await?;
            let is_active = test_http_tunnel(LOCALHOST_IP, *local_port).await?;
            open_tunnels_map.insert(
                *remote_port,
                Tunnel {
                    local_port: *local_port,
                    container_name: container_name.to_string(),
                    is_active,
                },
            );
            eprintln!(
                "Opened tunnel (endpoint: {}): {} -> {}",
                if is_active { "UP" } else { "DOWN" },
                local_socket,
                remote_socket
            );
        } else {
            let binding_pid = get_pid_using_port(&local_port.to_string()).await?;
            eprintln!(
                "Local port {} is already bound by another process: {:?}",
                local_port, binding_pid
            );
        }
    }
    Ok(())
}

pub async fn remote(ssh_url: &str, local_docker_host: &str) -> Result<(), AnyError> {
    let (sender, receiver) = mpsc::channel::<()>();

    let tx_mutex = Mutex::<Option<Sender<()>>>::new(Some(sender));

    ctrlc::set_handler(move || {
        if let Some(tx) = tx_mutex.lock().unwrap().take() {
            tx.send(()).unwrap();
        }
    })?;

    let re = Regex::new(r"^unix://").unwrap();
    let expanded_socket = shellexpand::tilde(&re.replace(&local_docker_host, "")).into_owned();
    let local_socket_path = Path::new(&expanded_socket);

    if let Some(path) = local_socket_path.parent() {
        fs::create_dir_all(path)?;
    }

    let mut builder = SessionBuilder::default();
    builder
        .known_hosts_check(KnownHosts::Strict)
        .connect_timeout(Duration::from_secs(5))
        .server_alive_interval(Duration::from_secs(5));

    let mut session = connect(&builder, ssh_url, local_socket_path).await?;

    let mut open_tunnels_map = HashMap::<u16, Tunnel>::new();

    loop {
        match session.check().await {
            Ok(_) => (),
            Err(_) => {
                eprintln!("SSH connection lost. Reconnecting...");
                match connect(&builder, ssh_url, local_socket_path).await {
                    Ok(s) => session = s,
                    Err(error) => {
                        eprintln!("ERROR: {}", error);
                        if let Some(()) = receiver.recv_timeout(Duration::from_secs(3)).ok() {
                            break;
                        }
                        continue;
                    }
                }
            }
        }
        let docker = match Docker::connect_with_local_defaults() {
            Ok(docker) => docker.with_timeout(Duration::from_secs(10)),
            Err(e) => {
                log::debug!("Failed connect to Docker API. Will retry: {}", e);
                continue;
            }
        };

        match manage_tunnels(&docker, &session, &mut open_tunnels_map).await {
            Ok(()) => (),
            Err(e) => {
                log::debug!("Connection failed. Will retry: {}", e);
                continue;
            }
        };
        if let Some(()) = receiver.recv_timeout(Duration::from_secs(10)).ok() {
            break;
        }
    }
    for (
        remote_port,
        Tunnel {
            local_port,
            container_name,
            ..
        },
    ) in open_tunnels_map
    {
        let local_socket = format!("{}:{}", LOCALHOST_IP, local_port);
        let remote_socket = format!("{}:{}", container_name, remote_port);
        close_port_forward(&session, local_port, remote_port).await?;
        println!("Closing: {} -> {}", local_socket, remote_socket);
    }
    // TODO: close forwarded socket
    session.close().await?;
    std::process::exit(0);
}
