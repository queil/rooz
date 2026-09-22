use crate::{
    model::types::VolumeSpec,
    util::{
        id::sanitize,
        labels::{
            CACHE_ROLE, DATA_ROLE, Labels, SSH_KEY_ROLE, SYSTEM_CONFIG_ROLE, WORK_ROLE,
            WORKSPACE_CONFIG_ROLE,
        },
    },
};
use bollard::models::{Mount, MountType};

pub const CACHE_SCOPE_ENV: &str = "ROOZ_CACHE_SCOPE";

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: getuid() is always successful and touches no memory
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

fn cache_scope_from(configured: Option<String>, uid: u32) -> String {
    match configured {
        Some(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => format!("uid-{}", uid),
    }
}

// Cache volumes are shared by path across a single operator's workspaces, which is the
// point of them. On an engine shared by several operators the path alone would also
// share them *between* operators, so the name carries a scope: the operator's uid by
// default, or an explicit key for teams whose local uids collide on one remote engine.
pub fn cache_scope() -> String {
    cache_scope_from(std::env::var(CACHE_SCOPE_ENV).ok(), current_uid())
}

pub fn cache_volume_name(scope: &str, path: &str) -> String {
    format!(
        "rooz_{}_{}_{}",
        sanitize(CACHE_ROLE),
        sanitize(scope),
        sanitize(path)
    )
}

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
pub struct VolumeFile {
    pub path: String,
    pub content: String,
    pub executable: bool,
    /// written owner-only; for files holding credentials or configuration
    pub private: bool,
}

impl VolumeFile {
    pub fn new(path: &str, content: &str) -> VolumeFile {
        VolumeFile {
            path: path.to_string(),
            content: content.to_string(),
            executable: false,
            private: false,
        }
    }

    pub fn new_private(path: &str, content: &str) -> VolumeFile {
        VolumeFile {
            private: true,
            ..VolumeFile::new(path, content)
        }
    }
}

impl std::fmt::Debug for VolumeFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VolumeFile")
            .field("path", &self.path)
            .field("content", &format!("<{} bytes>", self.content.len()))
            .field("executable", &self.executable)
            .field("private", &self.private)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct RoozVolume {
    pub path: String,
    pub role: RoozVolumeRole,
    pub sharing: RoozVolumeSharing,
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
            } => cache_volume_name(&cache_scope(), path),
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

    pub fn to_spec(&self) -> VolumeSpec {
        VolumeSpec {
            name: self.safe_volume_name(),
            labels: self.labels.clone(),
        }
    }

    pub fn work(key: &str, path: &str) -> RoozVolume {
        RoozVolume {
            path: path.into(),
            sharing: RoozVolumeSharing::Exclusive { key: key.into() },
            role: RoozVolumeRole::Work,
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
            labels: Some(Labels::from(&[Labels::role(
                RoozVolumeRole::Cache.as_str(),
            )])),
        }
    }

    pub fn config_data(
        workspace_key: &str,
        path: &str,
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
        RoozVolume {
            path: path.into(),
            role,
            sharing: RoozVolumeSharing::Exclusive {
                key: workspace_key.into(),
            },
            labels: Some(all_labels),
        }
    }

    pub fn workspace_config_read(workspace_key: &str, path: &str) -> RoozVolume {
        RoozVolume {
            path: path.into(),
            sharing: RoozVolumeSharing::Exclusive {
                key: workspace_key.to_string(),
            },
            role: RoozVolumeRole::WorkspaceConfig,
            labels: None,
        }
    }

    pub fn system_config(path: &str) -> RoozVolume {
        RoozVolume {
            path: path.into(),
            sharing: RoozVolumeSharing::Shared,
            role: RoozVolumeRole::SystemConfig,
            labels: Some(Labels::from(&[Labels::role(
                RoozVolumeRole::SystemConfig.as_str(),
            )])),
        }
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
    fn cache_name_carries_role_scope_and_path() {
        assert_eq!(
            cache_volume_name("uid-1000", "~/.cargo"),
            "rooz_cache_uid-1000_---cargo"
        );
    }

    #[test]
    fn cache_shared_uses_the_current_scope() {
        let name =
            vol(RoozVolumeRole::Cache, RoozVolumeSharing::Shared, "~/.cargo").safe_volume_name();
        assert_eq!(name, cache_volume_name(&cache_scope(), "~/.cargo"));
    }

    #[test]
    fn different_operators_get_different_cache_volumes() {
        assert_ne!(
            cache_volume_name("uid-1000", "~/.cargo"),
            cache_volume_name("uid-0", "~/.cargo")
        );
    }

    #[test]
    fn cache_scope_defaults_to_uid_and_honours_the_override() {
        assert_eq!(cache_scope_from(None, 1000), "uid-1000");
        assert_eq!(cache_scope_from(Some("   ".into()), 1000), "uid-1000");
        assert_eq!(cache_scope_from(Some("".into()), 0), "uid-0");
        assert_eq!(cache_scope_from(Some(" team-a ".into()), 1000), "team-a");
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
    fn config_data_workspace_config_name() {
        let v = RoozVolume::config_data(
            "ws",
            "/etc/rooz",
            None,
            Some(RoozVolumeRole::WorkspaceConfig),
        );
        assert_eq!(v.path, "/etc/rooz");
        assert_eq!(v.safe_volume_name(), "rooz-ws-workspace-config");
    }

    #[test]
    fn config_data_default_role_name() {
        let v = RoozVolume::config_data("ws", "/etc/rooz", None, None);
        assert_eq!(v.safe_volume_name(), "rooz_ws_-etc-rooz_data");
    }

    #[test]
    fn system_config_name() {
        let v = RoozVolume::system_config("/tmp/sys");
        assert_eq!(v.safe_volume_name(), "rooz_sys-config");
    }

    #[test]
    fn to_spec_carries_name_and_labels() {
        let v = RoozVolume::system_config("/tmp/sys");
        let spec = v.to_spec();
        assert_eq!(spec.name, "rooz_sys-config");
        assert!(spec.labels.is_some());
    }
}
