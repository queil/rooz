use bollard::service::SystemInfo;
use bollard::system::Version;
use bollard::Docker;

pub trait ContainerClient<'a> {
    fn client(&self) -> &Docker;
    fn backend(&self) -> &ContainerBackend;
}

pub struct ExecApi<'a> {
    pub client: &'a Client<'a>,
}

impl<'a> ContainerClient<'a> for ExecApi<'a> {
    fn client(&self) -> &Docker {
        self.client.client
    }

    fn backend(&self) -> &ContainerBackend {
        self.client.backend
    }
}

pub struct ImageApi<'a> {
    pub client: &'a Client<'a>,
}

impl<'a> ContainerClient<'a> for ImageApi<'a> {
    fn client(&self) -> &Docker {
        self.client.client
    }
    fn backend(&self) -> &ContainerBackend {
        self.client.backend
    }
}

pub struct Client<'a> {
    pub client: &'a Docker,
    pub backend: &'a ContainerBackend,
}

pub struct Api<'a> {
    pub exec: &'a ExecApi<'a>,
    pub image: &'a ImageApi<'a>,
    pub client: &'a Client<'a>,
}

impl<'a> ContainerClient<'a> for Api<'a> {
    fn client(&self) -> &Docker {
        self.client.client
    }
    fn backend(&self) -> &ContainerBackend {
        self.client.backend
    }
}

#[derive(Debug, Clone)]
pub enum ContainerBackend {
    DockerDesktop,
    RancherDesktop,
    Podman,
    Unknown,
}

impl ContainerBackend {
    pub async fn resolve(
        version: &Version,
        info: &SystemInfo,
    ) -> Result<Self, Box<dyn std::error::Error + 'static>> {
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
