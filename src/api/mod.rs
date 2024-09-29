use bollard::Docker;

use crate::util::backend::ContainerBackend;

pub mod container;
pub mod exec;
pub mod image;
pub mod sidecar;
pub mod volume;
pub mod workspace;

pub struct ImageApi<'a> {
    pub client: &'a Docker,
}

pub struct ExecApi<'a> {
    pub client: &'a Docker,
    pub backend: &'a ContainerBackend,
}

pub struct VolumeApi<'a> {
    pub client: &'a Docker,
}

pub struct ContainerApi<'a> {
    pub client: &'a Docker,
    pub backend: &'a ContainerBackend,
}

pub struct Api<'a> {
    pub exec: &'a ExecApi<'a>,
    pub image: &'a ImageApi<'a>,
    pub volume: &'a VolumeApi<'a>,
    pub container: &'a ContainerApi<'a>,
    pub client: &'a Docker,
}

pub struct GitApi<'a> {
    pub api: &'a Api<'a>,
}

pub struct WorkspaceApi<'a> {
    pub api: &'a Api<'a>,
    pub git: &'a GitApi<'a>,
}
