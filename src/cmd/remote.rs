use bollard::{
    container::ListContainersOptions,
    models::{Port, PortTypeEnum},
    Docker,
};
use openssh::{ForwardType, KnownHosts, SessionBuilder};
use regex::Regex;
use std::{
    collections::HashSet,
    fs,
    net::{Ipv4Addr, TcpListener},
    path::Path,
    sync::{
        mpsc::{self, Sender},
        Mutex,
    },
    time::Duration,
};

use crate::{labels, model::types::AnyError};

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

    if local_socket_path.exists() {
        fs::remove_file(local_socket_path)?;
    }

    let session = SessionBuilder::default()
        .known_hosts_check(KnownHosts::Accept)
        .connect_timeout(Duration::from_secs(5))
        .server_alive_interval(Duration::from_secs(60))
        .connect_mux(&ssh_url)
        .await?;

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

    let docker = Docker::connect_with_local_defaults()?;
    let mut tunnels = HashSet::<u16>::new();
    loop {
        let containers = docker
            .list_containers(Some(ListContainersOptions {
                filters: (&labels::Labels::new(None, None)).into(),
                ..Default::default()
            }))
            .await?;

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
                private_port,
                public_port,
                typ,
            } in ports
            {
                let public_port = public_port.unwrap_or(private_port);
                log::debug!(
                    "{} {} {} {} {}",
                    name,
                    ip.unwrap_or_default(),
                    private_port,
                    public_port,
                    typ.unwrap_or(PortTypeEnum::EMPTY)
                );

                let listen_socket = format!("127.0.0.1:{}", private_port);
                let connect_socket = format!("127.0.0.1:{}", public_port);
                session.check().await?;

                if !tunnels.contains(&public_port) {
                    if is_available(&private_port) {
                        session
                            .request_port_forward(
                                ForwardType::Local,
                                (Ipv4Addr::new(127, 0, 0, 1), private_port),
                                (Ipv4Addr::new(127, 0, 0, 1), public_port),
                            )
                            .await?;
                        println!(
                            "Forwarding: {} -> {} ({})",
                            listen_socket, connect_socket, name
                        );
                        tunnels.insert(public_port);
                    } else {
                        println!(
                            "Already bound, so maybe forwarding: {} -> {} ({})",
                            listen_socket, connect_socket, name
                        );
                        tunnels.insert(public_port);
                    }
                }
            }
        }
        if let Some(()) = receiver.recv_timeout(Duration::from_secs(10)).ok() {
            break;
        }
    }
    session.close().await?;
    std::process::exit(0);
}

fn is_available(port: &u16) -> bool {
    TcpListener::bind(("127.0.0.1", *port)).ok().is_some()
}
