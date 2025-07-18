use std::collections::HashMap;

use crate::util::id::to_safe_id;
use bollard::models::{Mount, MountTypeEnum};

#[derive(Debug, Clone)]
pub enum RoozVolumeSharing {
    Shared,
    Exclusive { key: String },
}

pub const HOME_ROLE: &'static str = "home";
pub const WORK_ROLE: &'static str = "work";
pub const CACHE_ROLE: &'static str = "cache";
pub const DATA_ROLE: &'static str = "data";
pub const SSH_KEY_ROLE: &'static str = "ssh-key";
pub const AGE_KEY_ROLE: &'static str = "age-key";
pub const SYSTEM_CONFIG_ROLE: &'static str = "sys-config";

#[derive(Debug, Clone)]
pub enum RoozVolumeRole {
    Home,
    Work,
    Cache,
    Data,
    SshKey,
    AgeKey,
    SystemConfig,
}

impl RoozVolumeRole {
    pub fn as_str(&self) -> &str {
        match self {
            RoozVolumeRole::Home => HOME_ROLE,
            RoozVolumeRole::Work => WORK_ROLE,
            RoozVolumeRole::Cache => CACHE_ROLE,
            RoozVolumeRole::Data => DATA_ROLE,
            RoozVolumeRole::SshKey => SSH_KEY_ROLE,
            RoozVolumeRole::AgeKey => AGE_KEY_ROLE,
            RoozVolumeRole::SystemConfig => SYSTEM_CONFIG_ROLE,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoozVolumeFile {
    pub file_path: String,
    pub data: String,
}

#[derive(Debug, Clone)]
pub struct RoozVolume {
    pub path: String,
    pub role: RoozVolumeRole,
    pub sharing: RoozVolumeSharing,
    pub files: Option<Vec<RoozVolumeFile>>,
}

impl RoozVolume {
    pub fn safe_volume_name(&self) -> String {
        let role_segment = to_safe_id(self.role.as_str());

        match self {
            RoozVolume {
                path,
                role: RoozVolumeRole::Data,
                sharing: RoozVolumeSharing::Exclusive { key },
                ..
            } => format!(
                "rooz_{}_{}_{}",
                to_safe_id(&key),
                to_safe_id(&path),
                &role_segment
            ),
            RoozVolume {
                path,
                sharing: RoozVolumeSharing::Shared,
                role: RoozVolumeRole::Cache,
                ..
            } => format!("rooz_{}_{}", &role_segment, to_safe_id(&path)),
            RoozVolume {
                sharing: RoozVolumeSharing::Exclusive { key },
                ..
            } => format!("rooz_{}_{}", to_safe_id(&key), &role_segment),
            RoozVolume { .. } => format!("rooz_{}", &role_segment),
        }
    }
    pub fn key(&self) -> Option<String> {
        match self {
            RoozVolume {
                sharing: RoozVolumeSharing::Exclusive { key },
                ..
            } => Some(key.to_string()),
            RoozVolume {
                role: RoozVolumeRole::Cache,
                ..
            } => Some("cache".into()),
            _ => None,
        }
    }

    pub fn is_exclusive(&self) -> bool {
        match self.sharing {
            RoozVolumeSharing::Exclusive { .. } => true,
            _ => false,
        }
    }

    fn expanded_path(&self, tilde_replacement: Option<&str>) -> String {
        match tilde_replacement {
            Some(replacement) => self.path.replace("~", &replacement),
            None => self.path.to_string(),
        }
    }

    pub fn to_mount(&self, tilde_replacement: Option<&str>) -> Mount {
        let vol_name = self.safe_volume_name();

        Mount {
            typ: Some(MountTypeEnum::VOLUME),
            source: Some(vol_name.into()),
            target: Some(self.expanded_path(tilde_replacement)),
            read_only: Some(false),
            ..Default::default()
        }
    }

    pub fn work(key: &str, path: &str) -> RoozVolume {
        RoozVolume {
            path: path.into(),
            sharing: RoozVolumeSharing::Exclusive { key: key.into() },
            role: RoozVolumeRole::Work,
            files: None,
        }
    }

    pub fn home(key: &str, path: &str) -> RoozVolume {
        RoozVolume {
            path: path.into(),
            sharing: RoozVolumeSharing::Exclusive { key: key.into() },
            role: RoozVolumeRole::Home,
            files: None,
        }
    }

    pub fn cache(path: &str) -> RoozVolume {
        RoozVolume {
            path: path.into(),
            sharing: RoozVolumeSharing::Shared,
            role: RoozVolumeRole::Cache,
            files: None,
        }
    }

    pub fn config_data(
        workspace_key: &str,
        path: &str,
        files: Option<HashMap<String, String>>,
    ) -> RoozVolume {
        match files {
            Some(files) => RoozVolume {
                path: path.to_string(),
                role: RoozVolumeRole::Data,
                sharing: RoozVolumeSharing::Exclusive {
                    key: workspace_key.into(),
                },
                files: Some(
                    files
                        .iter()
                        .map(|(file_name, data)| RoozVolumeFile {
                            file_path: file_name.to_string(),
                            data: data.to_string(),
                        })
                        .collect::<Vec<_>>(),
                ),
            },
            None => RoozVolume {
                path: path.into(),
                role: RoozVolumeRole::Data,
                sharing: RoozVolumeSharing::Exclusive {
                    key: workspace_key.into(),
                },
                files: None,
            },
        }
    }

    pub fn system_config(path: &str, data: Option<String>) -> RoozVolume {
        RoozVolume {
            path: path.into(),
            sharing: RoozVolumeSharing::Shared,
            role: RoozVolumeRole::SystemConfig,
            files: Some(vec![RoozVolumeFile {
                file_path: "rooz.config".to_string(),
                data: data.unwrap_or(
                    r#"gitconfig: |-
  [core]
    sshCommand = ssh -i /tmp/.ssh/id_ed25519 -o UserKnownHostsFile=/tmp/.ssh/known_hosts
                "#
                    .to_string(),
                ),
            }]),
        }
    }
}
