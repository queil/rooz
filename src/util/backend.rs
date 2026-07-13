use crate::model::types::AnyError;
use bollard::service::SystemInfo;
use bollard_stubs::models::SystemVersion;

fn parse_major_minor(v: &str) -> (u64, u64) {
    let mut parts = v.split('.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor)
}

pub fn check_version_floor(
    version: &SystemVersion,
    backend: &ContainerBackend,
) -> Result<(), AnyError> {
    let ver_str = version.version.as_deref().unwrap_or("");
    let (major, minor) = parse_major_minor(ver_str);

    let ok = match backend.engine {
        ContainerEngine::Podman => major >= 6,
        _ => major > 29 || (major == 29 && minor >= 5),
    };

    if !ok {
        return Err(format!("rooz requires Docker 29.5+ / Podman 6+ (found {})", ver_str).into());
    }
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use bollard_stubs::models::{SystemVersion, SystemVersionComponents};

    fn ver(os: &str, arch: &str) -> SystemVersion {
        SystemVersion {
            os: Some(os.to_string()),
            arch: Some(arch.to_string()),
            ..Default::default()
        }
    }

    fn ver_with_component(os: &str, arch: &str, component: &str) -> SystemVersion {
        SystemVersion {
            os: Some(os.to_string()),
            arch: Some(arch.to_string()),
            components: Some(vec![SystemVersionComponents {
                name: component.to_string(),
                version: "0".to_string(),
                details: None,
            }]),
            ..Default::default()
        }
    }

    fn info(operating_system: &str) -> bollard::service::SystemInfo {
        bollard::service::SystemInfo {
            operating_system: Some(operating_system.to_string()),
            ..Default::default()
        }
    }

    fn backend(engine: ContainerEngine) -> ContainerBackend {
        ContainerBackend {
            engine,
            platform: "linux/amd64".to_string(),
        }
    }

    fn ver_str(v: &str) -> SystemVersion {
        SystemVersion {
            version: Some(v.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn version_floor_docker() {
        let cases: &[(&str, bool)] = &[
            ("29.5.0", true),
            ("29.5.1", true),
            ("29.6.1", true),
            ("30.0.0", true),
            ("29.4.9", false),
            ("29.0.0", false),
            ("28.9.9", false),
            ("26.1.0", false),
            ("", false),
        ];
        for &(v, should_pass) in cases {
            let result = check_version_floor(&ver_str(v), &backend(ContainerEngine::Unknown));
            assert_eq!(
                result.is_ok(),
                should_pass,
                "docker floor check wrong for version '{}'",
                v
            );
        }
    }

    #[test]
    fn version_floor_podman() {
        let cases: &[(&str, bool)] = &[
            ("6.0.0", true),
            ("6.1.0", true),
            ("7.0.0", true),
            ("5.9.9", false),
            ("5.0.0", false),
            ("4.9.0", false),
            ("", false),
        ];
        for &(v, should_pass) in cases {
            let result = check_version_floor(&ver_str(v), &backend(ContainerEngine::Podman));
            assert_eq!(
                result.is_ok(),
                should_pass,
                "podman floor check wrong for version '{}'",
                v
            );
        }
    }

    #[test]
    fn version_floor_error_message_contains_phrase() {
        let err = check_version_floor(&ver_str("26.1.0"), &backend(ContainerEngine::Unknown))
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("rooz requires Docker 29.5+ / Podman 6+"),
            "unexpected error: {}",
            err
        );
    }

    #[tokio::test]
    async fn backend_detection_table() {
        use ContainerEngine::*;
        let cases: Vec<(SystemVersion, bollard::service::SystemInfo, ContainerEngine)> = vec![
            (ver("linux", "amd64"), info("Docker Desktop"), DockerDesktop),
            (
                ver("linux", "amd64"),
                info("Rancher Desktop WSL Distribution"),
                RancherDesktop,
            ),
            (
                ver_with_component("linux", "amd64", "Podman Engine"),
                info("linux"),
                Podman,
            ),
            (
                ver_with_component("linux", "amd64", "Engine"),
                info("linux"),
                Unknown,
            ),
            (ver("linux", "amd64"), info("Alpine Linux v3.20"), Unknown), // Colima
            (ver("linux", "aarch64"), info("OrbStack"), Unknown),         // OrbStack
        ];

        for (v, i, expected) in cases {
            let b = ContainerBackend::resolve(&v, &i).await.unwrap();
            assert_eq!(
                std::mem::discriminant(&b.engine),
                std::mem::discriminant(&expected),
                "wrong detection for operating_system={:?}",
                i.operating_system
            );
        }
    }

    #[tokio::test]
    async fn platform_is_os_slash_arch() {
        let b = ContainerBackend::resolve(&ver("linux", "amd64"), &info("Docker Desktop"))
            .await
            .unwrap();
        assert_eq!(b.platform, "linux/amd64");
    }
}
