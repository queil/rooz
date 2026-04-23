use crate::model::types::AnyError;
use bollard::service::SystemInfo;
use bollard_stubs::models::SystemVersion;

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

fn parse_version(v: &str) -> Option<(u64, u64)> {
    let mut parts = v.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor))
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

        let backend = backend(&info, &version);
        if let ContainerEngine::Unknown = backend.engine {
            log::debug!("{:?}", &version);
            log::debug!("{:?}", &backend);
        }

        // Subpath mounts require Docker >= 25.0 or Podman >= 4.7.
        let version_str = match backend.engine {
            ContainerEngine::Podman => version
                .components
                .as_ref()
                .and_then(|cs| cs.iter().find(|c| c.name == "Podman Engine"))
                .map(|c| c.version.as_str())
                .or_else(|| version.version.as_deref())
                .unwrap_or("0.0.0"),
            _ => version.version.as_deref().unwrap_or("0.0.0"),
        };

        let (req_major, req_minor) = match backend.engine {
            ContainerEngine::Podman => (4, 7),
            _ => (25, 0),
        };

        if let Some((major, minor)) = parse_version(version_str) {
            if major < req_major || (major == req_major && minor < req_minor) {
                let engine_name = match backend.engine {
                    ContainerEngine::Podman => "Podman",
                    _ => "Docker",
                };
                return Err(format!(
                    "rooz requires Docker >= 25.0 or Podman >= 4.7 for single-file mounts \
                     (detected {} {})",
                    engine_name, version_str
                )
                .into());
            }
        }

        Ok(backend)
    }
}
