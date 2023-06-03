use crate::id::to_safe_id;
use bollard::service::Mount;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct RoozCfg {
    pub shell: Option<String>,
    pub image: Option<String>,
    pub caches: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub enum ContainerResult {
    Created { id: String },
    AlreadyExists { id: String },
}

impl ContainerResult {
    pub fn id(&self) -> &str {
        match self {
            ContainerResult::Created { id } => &id,
            ContainerResult::AlreadyExists { id } => &id,
        }
    }
}

pub enum VolumeResult {
    Created,
    AlreadyExists,
}

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
    Git,
}

impl RoozVolumeRole {
    pub fn as_str(&self) -> &str {
        match self {
            RoozVolumeRole::Home => "home",
            RoozVolumeRole::Work => "work",
            RoozVolumeRole::Cache => "cache",
            RoozVolumeRole::Git => "git",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoozVolume {
    pub path: String,
    pub role: RoozVolumeRole,
    pub sharing: RoozVolumeSharing,
}

impl RoozVolume {
    pub fn safe_volume_name(&self) -> Result<String, Box<dyn std::error::Error + 'static>> {
        let safe_id = to_safe_id(self.role.as_str())?;

        let vol_name = match self {
            RoozVolume {
                sharing: RoozVolumeSharing::Exclusive { key },
                ..
            } => format!("rooz-{}-{}", to_safe_id(&key)?, &safe_id),
            RoozVolume {
                path: p,
                sharing: RoozVolumeSharing::Shared,
                role: RoozVolumeRole::Cache,
                ..
            } => format!("rooz-{}-{}", to_safe_id(&p)?, &safe_id),
            RoozVolume { .. } => format!("rooz-{}", &safe_id),
        };
        Ok(vol_name)
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
}

pub struct WorkSpec<'a> {
    pub image: &'a str,
    pub image_id: &'a str,
    pub shell: &'a str,
    pub uid: &'a str,
    pub user: &'a str,
    pub container_working_dir: &'a str,
    pub container_name: &'a str,
    pub workspace_key: &'a str,
    pub ephemeral: bool,
    pub git_vol_mount: Option<Mount>,
    pub caches: Option<Vec<String>>,
    pub privileged: bool,
    pub force_recreate: bool,
}

pub struct RunSpec<'a> {
    pub reason: &'a str,
    pub image: &'a str,
    pub image_id: &'a str,
    pub user: Option<&'a str>,
    pub work_dir: Option<&'a str>,
    pub container_name: &'a str,
    pub workspace_key: &'a str,
    pub mounts: Option<Vec<Mount>>,
    pub entrypoint: Option<Vec<&'a str>>,
    pub privileged: bool,
    pub force_recreate: bool,
    pub auto_remove: bool,
}
