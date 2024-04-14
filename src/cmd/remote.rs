use futures::channel::oneshot::{self, Sender};
use openssh::{ForwardType, KnownHosts, SessionBuilder};
use regex::Regex;
use std::{fs, path::Path, sync::Mutex, time::Duration};

use crate::model::types::AnyError;

pub async fn remote(ssh_url: &str, local_docker_host: &str) -> Result<(), AnyError> {
    let (sender, receiver) = oneshot::channel::<()>();

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
        .connect(&ssh_url)
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

    futures::executor::block_on(receiver)?;

    if local_socket_path.exists() {
        fs::remove_file(local_socket_path)?;
    }
    std::process::exit(0);
}
