use bollard::{
    container::ListContainersOptions,
    models::{Port, PortTypeEnum},
    Docker,
};

use openssh::{ForwardType, KnownHosts, Session, SessionBuilder};
use regex::Regex;
use std::{
    collections::HashMap,
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

    let remote_socket = Path::new(&socket_url);

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
    log::debug!("Testing tunnel response: {}", response_str);
    Ok(response_str.starts_with("HTTP/1.1") || response_str.starts_with("HTTP/2.0"))
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

async fn close_tunnel(
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

async fn get_multiplexer_pid(session: &Session) -> Option<u32> {
    let ctl_path = session.control_socket();

    let mut system = sysinfo::System::new();
    system.refresh_all();

    for (pid, process) in system.processes() {
        if process.cmd().contains(&ctl_path.as_os_str().to_os_string()) {
            return Some(pid.as_u32());
        }
    }
    None
}

fn is_available(port: &u16) -> bool {
    TcpListener::bind(("127.0.0.1", *port)).ok().is_some()
}

pub async fn remote(ssh_url: &str, local_docker_host: &str) -> Result<(), AnyError> {
    const LOCALHOST_IP: &str = "127.0.0.1";
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

    let mut open_tunnels = HashMap::<u16, (u16, String)>::new();

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
        let multiplexer_pid = get_multiplexer_pid(&session).await;
        let docker = match Docker::connect_with_local_defaults() {
            Ok(docker) => docker.with_timeout(Duration::from_secs(10)),
            Err(e) => {
                log::debug!("Failed connect to Docker API. Will retry: {}", e);
                continue;
            }
        };
        let containers = match docker
            .list_containers(Some(ListContainersOptions {
                filters: (&labels::Labels::default()).into(),
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

        for (name, ports) in containers.iter().map(|c| {
            let names = c
                .names
                .as_ref()
                .map(|n| n.concat())
                .unwrap_or(c.id.as_ref().unwrap().to_string());
            let ports = c.clone().ports.unwrap_or(Vec::<_>::new());
            (names.to_string(), ports)
        }) {
            for Port {
                ip,
                private_port: local_port,
                public_port,
                typ,
            } in ports
            {
                let remote_port = public_port.unwrap_or(local_port);
                let mut name = name.to_string();
                if let Some(stripped) = name.strip_prefix("/") {
                    name = stripped.to_string();
                }
                log::debug!(
                    "{} {} {} {} {}",
                    name,
                    ip.unwrap_or_default(),
                    local_port,
                    remote_port,
                    typ.unwrap_or(PortTypeEnum::EMPTY)
                );

                let local_socket = format!("{}:{}", LOCALHOST_IP, local_port);
                let remote_socket = format!("{}:{}", name, remote_port);

                if open_tunnels.contains_key(&remote_port)
                    && !test_http_tunnel(LOCALHOST_IP, local_port).await?
                {
                    close_tunnel(&session, local_port, remote_port).await?;
                    open_tunnels.remove(&remote_port);
                    println!("Removed dead tunnel: {} -> {}", local_socket, remote_socket);
                }

                if is_available(&local_port) {
                    open_tunnel(&session, local_port, remote_port).await?;

                    if test_http_tunnel(LOCALHOST_IP, local_port).await? {
                        println!("Opened tunnel: {} -> {}", local_socket, remote_socket);
                        open_tunnels.insert(remote_port, (local_port, name.to_string()));
                    } else {
                        log::debug!("No response. Closing the tunnel");
                        close_tunnel(&session, local_port, remote_port).await?;
                    }
                } else {
                    match (
                        get_pid_using_port(&local_port.to_string()).await?,
                        multiplexer_pid,
                    ) {
                        (Some(pid), Some(mux_pid)) if pid == mux_pid => (),
                        (Some(pid), Some(mux_pid)) => {
                            eprintln!(
                                "Local port {} is already bound by another process: {}. Mux pid: {}",
                                local_port, pid, mux_pid
                            );
                        }
                        (_, _) => {
                            log::debug!(
                                "Local port {} is already bound by another process: UNKNOWN",
                                local_port,
                            );
                        }
                    };
                }
            }
        }
        if let Some(()) = receiver.recv_timeout(Duration::from_secs(10)).ok() {
            break;
        }
    }
    for (remote_port, (local_port, name)) in open_tunnels {
        let local_socket = format!("{}:{}", LOCALHOST_IP, local_port);
        let remote_socket = format!("{}:{}", name, remote_port);
        close_tunnel(&session, local_port, remote_port).await?;
        println!("Closing: {} -> {}", local_socket, remote_socket);
    }
    // TODO: close forwarded socket
    session.close().await?;
    std::process::exit(0);
}
