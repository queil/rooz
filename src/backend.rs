use bollard::service::SystemInfo;
use bollard::system::Version;
use bollard::Docker;

use crate::types::AnyError;

pub struct ExecApi<'a> {
    pub client: &'a Docker,
    pub backend: &'a ContainerBackend,
}

pub struct ImageApi<'a> {
    pub client: &'a Docker,
    pub backend: &'a ContainerBackend,
}

pub struct VolumeApi<'a> {
    pub client: &'a Docker,
    pub backend: &'a ContainerBackend,
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
    pub backend: &'a ContainerBackend,
}

pub struct GitApi<'a> {
    pub api: &'a Api<'a>,
}

pub struct WorkspaceApi<'a> {
    pub api: &'a Api<'a>,
    pub git: &'a GitApi<'a>,
}

#[derive(Debug, Clone)]
pub enum ContainerBackend {
    DockerDesktop,
    RancherDesktop,
    Podman,
    Unknown,
}

impl ContainerBackend {
    pub async fn resolve(version: &Version, info: &SystemInfo) -> Result<Self, AnyError> {
        fn backend(info: &SystemInfo, version: &Version) -> ContainerBackend {
            if let SystemInfo {
                operating_system: Some(name),
                ..
            } = &info
            {
                match name.as_str() {
                    "Rancher Desktop WSL Distribution" => ContainerBackend::RancherDesktop,
                    "Docker Desktop" => ContainerBackend::DockerDesktop,
                    _ => {
                        if let Some(components) = &version.components {
                            if components.iter().any(|c| c.name == "Podman Engine") {
                                ContainerBackend::Podman
                            } else {
                                ContainerBackend::Unknown
                            }
                        } else {
                            ContainerBackend::Unknown
                        }
                    }
                }
            } else {
                ContainerBackend::Unknown
            }
        }

        let backend = backend(&info, &version);
        if let ContainerBackend::Unknown = backend {
            log::debug!("{:?}", &version);
            log::debug!("{:?}", &info);
        }
        Ok(backend)
    }
}
