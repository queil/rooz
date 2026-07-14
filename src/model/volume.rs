use std::collections::HashMap;

use crate::{
    config::config::SystemConfig,
    model::types::AnyError,
    util::{
        id::sanitize,
        labels::{
            CACHE_ROLE, DATA_ROLE, Labels, SSH_KEY_ROLE, SYSTEM_CONFIG_ROLE, WORK_ROLE,
            WORKSPACE_CONFIG_ROLE,
        },
    },
};
use bollard::models::{Mount, MountType};

#[derive(Debug, Clone)]
pub enum RoozVolumeSharing {
    Shared,
    Exclusive { key: String },
}

#[derive(Debug, Clone)]
pub enum RoozVolumeRole {
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
            RoozVolumeRole::Work => WORK_ROLE,
            RoozVolumeRole::Cache => CACHE_ROLE,
            RoozVolumeRole::Data => DATA_ROLE,
            RoozVolumeRole::SshKey => SSH_KEY_ROLE,
            RoozVolumeRole::WorkspaceConfig => WORKSPACE_CONFIG_ROLE,
            RoozVolumeRole::SystemConfig => SYSTEM_CONFIG_ROLE,
        }
    }
}

#[derive(Clone)]
pub struct RoozVolumeFile {
    pub file_path: String,
    pub data: String,
}

impl std::fmt::Debug for RoozVolumeFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoozVolumeFile")
            .field("file_path", &self.file_path)
            .field("data", &format!("<{} bytes>", self.data.len()))
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct RoozVolume {
    pub path: String,
    pub role: RoozVolumeRole,
    pub sharing: RoozVolumeSharing,
    pub files: Option<Vec<RoozVolumeFile>>,
    pub labels: Option<Labels>,
}

impl RoozVolume {
    pub fn safe_volume_name(&self) -> String {
        let role_segment = sanitize(self.role.as_str());

        match self {
            RoozVolume {
                path,
                role: RoozVolumeRole::Data,
                sharing: RoozVolumeSharing::Exclusive { key },
                ..
            } => format!(
                "rooz_{}_{}_{}",
                sanitize(&key),
                sanitize(&path),
                &role_segment
            ),
            RoozVolume {
                path,
                sharing: RoozVolumeSharing::Shared,
                role: RoozVolumeRole::Cache,
                ..
            } => format!("rooz_{}_{}", &role_segment, sanitize(&path)),
            RoozVolume {
                sharing: RoozVolumeSharing::Exclusive { key },
                ..
            } => format!("rooz-{}-{}", sanitize(&key), &role_segment),
            RoozVolume { .. } => format!("rooz_{}", &role_segment),
        }
    }

    pub fn expanded_path(&self, tilde_replacement: Option<&str>) -> String {
        match tilde_replacement {
            Some(replacement) => self.path.replace("~", &replacement),
            None => self.path.to_string(),
        }
    }

    pub fn to_mount(&self, tilde_replacement: Option<&str>) -> Mount {
        let vol_name = self.safe_volume_name();

        Mount {
            typ: Some(MountType::VOLUME),
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
            labels: Some(Labels::from(&[
                Labels::workspace(key),
                Labels::role(RoozVolumeRole::Work.as_str()),
            ])),
        }
    }

    pub fn cache(path: &str) -> RoozVolume {
        RoozVolume {
            path: path.into(),
            sharing: RoozVolumeSharing::Shared,
            role: RoozVolumeRole::Cache,
            files: None,
            labels: Some(Labels::from(&[Labels::role(
                RoozVolumeRole::Cache.as_str(),
            )])),
        }
    }

    pub fn config_data(
        workspace_key: &str,
        path: &str,
        files: Option<HashMap<String, String>>,
        labels: Option<Labels>,
        role: Option<RoozVolumeRole>,
    ) -> RoozVolume {
        let role = role.unwrap_or(RoozVolumeRole::Data);
        let mut all_labels = Labels::from(&[
            Labels::workspace(workspace_key),
            Labels::role(role.as_str()),
        ]);

        if let Some(items) = labels {
            all_labels.extend_with_labels(items);
        }
        match files {
            Some(files) => RoozVolume {
                path: path.to_string(),
                role: role,
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
                labels: Some(all_labels),
            },
            None => RoozVolume {
                path: path.into(),
                role,
                sharing: RoozVolumeSharing::Exclusive {
                    key: workspace_key.into(),
                },
                files: None,
                labels: Some(all_labels),
            },
        }
    }

    pub fn workspace_config_read(workspace_key: &str, path: &str) -> RoozVolume {
        RoozVolume {
            path: path.into(),
            sharing: RoozVolumeSharing::Exclusive {
                key: workspace_key.to_string(),
            },
            role: RoozVolumeRole::WorkspaceConfig,
            files: None,
            labels: None,
        }
    }

    pub fn system_config_read(path: &str) -> RoozVolume {
        RoozVolume {
            path: path.into(),
            sharing: RoozVolumeSharing::Shared,
            role: RoozVolumeRole::SystemConfig,
            files: None,
            labels: Some(Labels::from(&[Labels::role(
                RoozVolumeRole::SystemConfig.as_str(),
            )])),
        }
    }

    pub fn system_config(path: &str, data: String) -> RoozVolume {
        RoozVolume {
            path: path.into(),
            sharing: RoozVolumeSharing::Shared,
            role: RoozVolumeRole::SystemConfig,
            files: Some(vec![RoozVolumeFile {
                file_path: "rooz.config".to_string(),
                data: data,
            }]),
            labels: Some(Labels::from(&[Labels::role(
                RoozVolumeRole::SystemConfig.as_str(),
            )])),
        }
    }

    pub fn system_config_init(path: &str, data: SystemConfig) -> Result<RoozVolume, AnyError> {
        Ok(RoozVolume::system_config(
            path,
            SystemConfig::to_string(&data)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(role: RoozVolumeRole, sharing: RoozVolumeSharing, path: &str) -> RoozVolume {
        RoozVolume {
            path: path.to_string(),
            role,
            sharing,
            files: None,
            labels: None,
        }
    }

    #[test]
    fn work_exclusive_uses_dashes() {
        let name = vol(
            RoozVolumeRole::Work,
            RoozVolumeSharing::Exclusive {
                key: "my-ws".to_string(),
            },
            "/work",
        )
        .safe_volume_name();
        assert_eq!(name, "rooz-my-ws-work");
    }

    #[test]
    fn cache_shared_uses_underscores() {
        let name =
            vol(RoozVolumeRole::Cache, RoozVolumeSharing::Shared, "~/.cargo").safe_volume_name();
        assert_eq!(name, "rooz_cache_---cargo");
    }

    #[test]
    fn data_exclusive_uses_underscores() {
        let name = vol(
            RoozVolumeRole::Data,
            RoozVolumeSharing::Exclusive {
                key: "ws1".to_string(),
            },
            "/data/stuff",
        )
        .safe_volume_name();
        assert_eq!(name, "rooz_ws1_-data-stuff_data");
    }

    #[test]
    fn shared_non_cache_uses_underscores() {
        let name = vol(
            RoozVolumeRole::SystemConfig,
            RoozVolumeSharing::Shared,
            "/tmp/sys",
        )
        .safe_volume_name();
        assert_eq!(name, "rooz_sys-config");
    }

    #[test]
    fn sanitize_path_collision_pinned() {
        // ~/a.txt and ~/a_txt collide after sanitize — pinned known wart
        let a = vol(RoozVolumeRole::Cache, RoozVolumeSharing::Shared, "~/a.txt").safe_volume_name();
        let b = vol(RoozVolumeRole::Cache, RoozVolumeSharing::Shared, "~/a_txt").safe_volume_name();
        assert_eq!(a, b);
    }

    #[test]
    fn workspace_config_exclusive_name() {
        let name = vol(
            RoozVolumeRole::WorkspaceConfig,
            RoozVolumeSharing::Exclusive {
                key: "xr7".to_string(),
            },
            "/etc/rooz",
        )
        .safe_volume_name();
        assert_eq!(name, "rooz-xr7-workspace-config");
    }

    #[test]
    fn to_mount_expands_tilde() {
        let mount = vol(
            RoozVolumeRole::Work,
            RoozVolumeSharing::Exclusive {
                key: "ws".to_string(),
            },
            "~/work",
        )
        .to_mount(Some("/home/user"));
        assert_eq!(mount.target.as_deref(), Some("/home/user/work"));
        assert_eq!(mount.source.as_deref(), Some("rooz-ws-work"));
    }

    #[test]
    fn to_mount_keeps_tilde_without_replacement() {
        let mount = vol(
            RoozVolumeRole::Work,
            RoozVolumeSharing::Exclusive {
                key: "ws".to_string(),
            },
            "~/work",
        )
        .to_mount(None);
        assert_eq!(mount.target.as_deref(), Some("~/work"));
    }

    #[test]
    fn config_data_builds_files() {
        let mut files = HashMap::new();
        files.insert(
            "workspace.config".to_string(),
            "image: alpine\n".to_string(),
        );

        let v = RoozVolume::config_data(
            "ws",
            "/etc/rooz",
            Some(files),
            None,
            Some(RoozVolumeRole::WorkspaceConfig),
        );

        assert_eq!(v.path, "/etc/rooz");
        assert_eq!(v.safe_volume_name(), "rooz-ws-workspace-config");
        let files = v.files.expect("files expected");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_path, "workspace.config");
        // data is stored verbatim here; trimming happens later in ensure_file
        assert_eq!(files[0].data, "image: alpine\n");
    }

    #[test]
    fn config_data_without_files() {
        let v = RoozVolume::config_data("ws", "/etc/rooz", None, None, None);
        assert!(v.files.is_none());
        assert_eq!(v.safe_volume_name(), "rooz_ws_-etc-rooz_data");
    }

    #[test]
    fn system_config_carries_rooz_config_file() {
        let v = RoozVolume::system_config("/tmp/sys", "cfg-body".to_string());
        assert_eq!(v.safe_volume_name(), "rooz_sys-config");
        let files = v.files.expect("files expected");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_path, "rooz.config");
        assert_eq!(files[0].data, "cfg-body");
    }
}
