use crate::model::types::AnyError;
use bollard::{secret::SystemVersion, service::SystemInfo};

#[derive(Debug, Clone)]
pub enum ContainerEngine {
    DockerDesktop,
    RancherDesktop,
    Podman,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct ContainerBackend {
    pub engine: ContainerEngine,
    pub platform: String,
}

impl ContainerBackend {
    pub async fn resolve(version: &SystemVersion, info: &SystemInfo) -> Result<Self, AnyError> {
        fn backend(info: &SystemInfo, version: &SystemVersion) -> ContainerBackend {
            if let SystemInfo {
                operating_system: Some(name),
                ..
            } = &info
            {
                let os = &version.os.as_deref().unwrap();
                let arch = &version.arch.as_deref().unwrap();
                let platform = format!("{}/{}", os, arch);

                match name.as_str() {
                    "Rancher Desktop WSL Distribution" => ContainerBackend {
                        engine: ContainerEngine::RancherDesktop,
                        platform: platform.to_string(),
                    },
                    "Docker Desktop" => ContainerBackend {
                        engine: ContainerEngine::DockerDesktop,
                        platform: platform.to_string(),
                    },
                    _ => {
                        if let Some(components) = &version.components {
                            if components.iter().any(|c| c.name == "Podman Engine") {
                                ContainerBackend {
                                    engine: ContainerEngine::Podman,
                                    platform: platform.to_string(),
                                }
                            } else {
                                ContainerBackend {
                                    engine: ContainerEngine::Unknown,
                                    platform: platform.to_string(),
                                }
                            }
                        } else {
                            ContainerBackend {
                                engine: ContainerEngine::Unknown,
                                platform: platform.to_string(),
                            }
                        }
                    }
                }
            } else {
                ContainerBackend {
                    engine: ContainerEngine::Unknown,
                    platform: "unknown".to_string(),
                }
            }
        }

        let info = backend(&info, &version);
        if let ContainerEngine::Unknown = info.engine {
            log::debug!("{:?}", &version);
            log::debug!("{:?}", &info);
        }
        Ok(info)
    }
}
