use bollard::Docker;

use crate::{config::config::SystemConfig, util::backend::ContainerBackend};

pub mod config;
pub mod container;
pub mod crypt;
pub mod exec;
pub mod image;
pub mod sidecar;
pub mod volume;
pub mod workspace;

pub struct ImageApi<'a> {
    pub client: &'a Docker,
    pub backend: &'a ContainerBackend,
}

pub struct ExecApi<'a> {
    pub client: &'a Docker,
    pub backend: &'a ContainerBackend,
}

pub struct ContainerApi<'a> {
    pub client: &'a Docker,
    pub image: &'a ImageApi<'a>,
    pub backend: &'a ContainerBackend,
}

pub struct VolumeApi<'a> {
    pub client: &'a Docker,
    pub container: &'a ContainerApi<'a>,
}

pub struct CryptApi {}

pub struct Api<'a> {
    pub exec: &'a ExecApi<'a>,
    pub image: &'a ImageApi<'a>,
    pub volume: &'a VolumeApi<'a>,
    pub container: &'a ContainerApi<'a>,
    pub system_config: &'a SystemConfig,
    pub client: &'a Docker,
}

pub struct GitApi<'a> {
    pub api: &'a Api<'a>,
}

pub struct ConfigApi<'a> {
    pub api: &'a Api<'a>,
    pub crypt: &'a CryptApi,
}

pub struct WorkspaceApi<'a> {
    pub api: &'a Api<'a>,
    pub git: &'a GitApi<'a>,
    pub config: &'a ConfigApi<'a>,
    pub crypt: &'a CryptApi,
}
