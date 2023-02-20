use bollard::container::LogOutput::Console;
use bollard::container::{Config, CreateContainerOptions, LogsOptions, StartContainerOptions};
use bollard::errors::Error::DockerResponseServerError;
use bollard::models::MountTypeEnum::VOLUME;
use bollard::models::{HostConfig, Mount};
use bollard::volume::CreateVolumeOptions;
use bollard::Docker;
use futures::stream::StreamExt;
use std::collections::HashMap;
use std::io::{self, Write};

use base64::{engine::general_purpose, Engine as _};

//TODO: identify what resources should be persisted in volumes (like SSH keys)
//TODO: tinker with different workflows: i.e. ephemeral - clone-develop-destroy
//TODO: configuration (allow using custom images) and CLI
//TODO: for POC we run as root, then let's figure out how we can reliably ensure a non-root user regardless of the Linux distro in all launched containers

#[tokio::main]
async fn main() {
    let init_image = "alpine/git:latest".to_string();
    let init_container_name = "init".to_string(); //randomize this
    let ssh_keys_volume_name = "rooz-ssh-keys".to_string();

    let docker = match Docker::connect_with_local_defaults() {
        Ok(docker) => docker,
        Err(e) => panic!("{}", e),
    };

    // volumes
    let ssh_keys_vol_options = CreateVolumeOptions::<String> {
        name: ssh_keys_volume_name.clone(),
        labels: HashMap::from([("io.rooz.role".to_string(), "ssh-keys".to_string())]),
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

    // create and start container

    let options = CreateContainerOptions {
        name: &init_container_name, // this needs to be random
        platform: None,
    };

    let host_config = HostConfig {
        auto_remove: Some(true),
        mounts: Some(vec![Mount {
            typ: Some(VOLUME),
            source: Some(ssh_keys_volume_name.clone()),
            read_only: Some(false),
            target: Some("/root/.ssh".to_string()),
            ..Default::default()
        }]),
        ..Default::default()
    };

    let init_script = general_purpose::STANDARD.encode(
        r#"
    KEYFILE='/root/.ssh/id_ed25519'
    ls "$KEYFILE.pub" || ssh-keygen -t ed25519 -N '' -f $KEYFILE
    cat "$KEYFILE.pub"
    "#,
    );

    let inject = format!(
        "echo '{}' | base64 -d > /entrypoint.sh && chmod +x /entrypoint.sh && /entrypoint.sh",
        &init_script
    );
    let init_entrypoint = ["sh", "-c", &inject];

    let config = Config {
        image: Some(init_image),
        entrypoint: Some(init_entrypoint.map(|x| x.to_string()).to_vec()),
        working_dir: Some(String::from("/build")),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        host_config: Some(host_config),
        tty: Some(true),
        ..Default::default()
    };

    let _x = match docker.create_container(Some(options), config).await {
        Ok(response) => response,
        Err(e) => panic!("{}", e),
    };

    let log_options = LogsOptions::<String> {
        stdout: true,
        follow: true,
        ..Default::default()
    };

    let _z = match docker
        .start_container(&init_container_name, None::<StartContainerOptions<String>>)
        .await
    {
        Ok(_) => (),
        Err(e) => panic!("{}", e),
    };

    let mut stream = docker.logs(&init_container_name, Some(log_options));

    while let Some(l) = stream.next().await {
        match l {
            Ok(Console { message: m }) => io::stdout().write_all(&m).expect("Write to stdout"),
            Ok(msg) => panic!("{}", msg),
            Err(e) => panic!("{}", e),
        };
    }
}
