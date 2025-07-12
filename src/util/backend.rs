use crate::model::types::AnyError;
use bollard::{secret::SystemVersion, service::SystemInfo};


#[derive(Debug, Clone)]
pub enum ContainerBackend {
    DockerDesktop,
    RancherDesktop,
    Podman,
    Unknown,
}

impl ContainerBackend {
    pub async fn resolve(version: &SystemVersion, info: &SystemInfo) -> Result<Self, AnyError> {
        fn backend(info: &SystemInfo, version: &SystemVersion) -> ContainerBackend {
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
