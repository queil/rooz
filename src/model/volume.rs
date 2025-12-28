use std::collections::HashMap;

use crate::model::volume::RoozVolumeRole::WorkspaceConfig;
use crate::{
    config::config::SystemConfig,
    model::types::AnyError,
    util::{
        id::to_safe_id,
        labels::{
            CACHE_ROLE, DATA_ROLE, HOME_ROLE, Labels, SSH_KEY_ROLE, SYSTEM_CONFIG_ROLE, WORK_ROLE,
            WORKSPACE_CONFIG_ROLE,
        },
    },
};
use bollard::models::{Mount, MountTypeEnum};

#[derive(Debug, Clone)]
pub enum RoozVolumeSharing {
    Shared,
    Exclusive { key: String },
}

#[derive(Debug, Clone)]
pub enum RoozVolumeRole {
    Home,
    Work,
    Cache,
    Data,
    SshKey,
    WorkspaceConfig,
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
            RoozVolumeRole::WorkspaceConfig => WORKSPACE_CONFIG_ROLE,
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
pub struct VolumeBackedPath {
    pub path: String,
    pub role: RoozVolumeRole,
    pub sharing: RoozVolumeSharing,
    pub files: Option<Vec<RoozVolumeFile>>,
    pub labels: Option<Labels>,
}

impl VolumeBackedPath {
    pub fn safe_volume_name(&self) -> String {
        let role_segment = to_safe_id(self.role.as_str());

        match self {
            VolumeBackedPath {
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
            VolumeBackedPath {
                path,
                sharing: RoozVolumeSharing::Shared,
                role: RoozVolumeRole::Cache,
                ..
            } => format!("rooz_{}_{}", &role_segment, to_safe_id(&path)),
            VolumeBackedPath {
                sharing: RoozVolumeSharing::Exclusive { key },
                ..
            } => format!("rooz_{}_{}", to_safe_id(&key), &role_segment),
            VolumeBackedPath { .. } => format!("rooz_{}", &role_segment),
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

    fn exclusive(key: &str, path: &str, role: RoozVolumeRole) -> VolumeBackedPath {
        VolumeBackedPath {
            path: path.into(),
            sharing: RoozVolumeSharing::Exclusive { key: key.into() },
            role: role.clone(),
            files: None,
            labels: Some(Labels::from(&[
                Labels::workspace(key),
                Labels::role(role.as_str()),
            ])),
        }
    }

    fn shared(path: &str, role: RoozVolumeRole) -> VolumeBackedPath {
        VolumeBackedPath {
            path: path.into(),
            sharing: RoozVolumeSharing::Shared,
            role: role.clone(),
            files: None,
            labels: Some(Labels::from(&[Labels::role(role.as_str())])),
        }
    }

    pub fn home(key: &str, path: &str) -> VolumeBackedPath {
        VolumeBackedPath::exclusive(key, path, RoozVolumeRole::Home)
    }

    pub fn work(key: &str, path: &str) -> VolumeBackedPath {
        VolumeBackedPath::exclusive(key, path, RoozVolumeRole::Work)
    }

    pub fn cache(path: &str) -> VolumeBackedPath {
        VolumeBackedPath::shared(path, RoozVolumeRole::Cache)
    }

    pub fn system_config_read(path: &str) -> VolumeBackedPath {
        VolumeBackedPath::shared(path, RoozVolumeRole::SystemConfig)
    }

    pub fn config_data(
        workspace_key: &str,
        path: &str,
        files: Option<HashMap<String, String>>,
        labels: Option<Labels>,
        role: Option<RoozVolumeRole>,
    ) -> VolumeBackedPath {
        let role = role.unwrap_or(RoozVolumeRole::Data);

        let vbp = {
            let default = VolumeBackedPath::exclusive(path, workspace_key, role.clone());
            VolumeBackedPath {
                labels: {
                    let mut all_labels = if let Some(custom_labels) = labels {
                        custom_labels
                    } else {
                        Labels::from(&[])
                    };
                    if let Some(ls) = default.labels {
                        all_labels.extend_with_labels(ls);
                    }

                    Some(all_labels)
                },
                ..default
            }
        };

        match files {
            Some(files) => VolumeBackedPath {
                files: Some(
                    files
                        .iter()
                        .map(|(file_name, data)| RoozVolumeFile {
                            file_path: file_name.to_string(),
                            data: data.to_string(),
                        })
                        .collect::<Vec<_>>(),
                ),
                ..vbp
            },
            None => vbp,
        }
    }

    pub fn workspace_config_read(workspace_key: &str, path: &str) -> VolumeBackedPath {
        VolumeBackedPath {
            labels: None,
            ..VolumeBackedPath::exclusive(workspace_key, path, WorkspaceConfig)
        }
    }

    pub fn system_config(path: &str, data: String) -> VolumeBackedPath {
        VolumeBackedPath {
            files: Some(vec![RoozVolumeFile {
                file_path: "rooz.config".to_string(),
                data,
            }]),
            ..VolumeBackedPath::shared(path, RoozVolumeRole::SystemConfig)
        }
    }

    pub fn system_config_init(
        path: &str,
        data: SystemConfig,
    ) -> Result<VolumeBackedPath, AnyError> {
        Ok(VolumeBackedPath::system_config(
            path,
            SystemConfig::to_string(&data)?,
        ))
    }
}
