use bollard::Docker;
use bollard::service::SystemInfo;
use bollard::system::Version;


pub struct ExecApi<'a> {
  pub client: &'a Docker,
}

pub struct Api<'a> {
  pub client: &'a Docker,
  pub backend: ContainerBackend,
  pub exec: &'a ExecApi<'a>,
}

#[derive(Debug)]
pub enum ContainerBackend {
    DockerDesktop,
    RancherDesktop,
    Podman,
    Unknown,
}

impl ContainerBackend {
    pub async fn resolve(version: &Version, info: &SystemInfo) -> Result<Self, Box<dyn std::error::Error + 'static>> {
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