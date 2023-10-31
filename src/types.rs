use std::{collections::HashMap, fs};

use crate::{cli::WorkParams, constants, id::to_safe_id};
use bollard::service::Mount;
use serde::Deserialize;

pub type AnyError = Box<dyn std::error::Error + 'static>;

#[derive(Debug, Deserialize, Clone)]
pub struct RoozSidecar {
    pub image: String,
    pub env: Option<HashMap<String, String>>,
    pub command: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RoozCfg {
    pub shell: Option<String>,
    pub image: Option<String>,
    pub user: Option<String>,
    pub caches: Option<Vec<String>>,
    pub sidecars: Option<HashMap<String, RoozSidecar>>,
}

impl RoozCfg {
    pub fn from_file(path: &str) -> Result<RoozCfg, Box<dyn std::error::Error + 'static>> {
        Self::from_string(fs::read_to_string(path)?)
    }

    pub fn from_string(config: String) -> Result<RoozCfg, Box<dyn std::error::Error + 'static>> {
        let f = RoozCfg::deserialize(toml::de::Deserializer::new(&config));
        match f {
            Ok(val) => Ok(val),
            Err(e) => Err(Box::new(e)),
        }
    }

    pub fn shell(
        cli: &WorkParams,
        cli_cfg: &Option<RoozCfg>,
        repo_cfg: &Option<RoozCfg>,
    ) -> String {
        cli.shell
            .clone()
            .or(cli_cfg.clone().map(|c| c.shell).flatten())
            .or(repo_cfg.clone().map(|c| c.shell).flatten())
            .or(cli.env_shell.clone())
            .unwrap_or(constants::DEFAULT_SHELL.into())
    }

    pub fn image(
        cli: &WorkParams,
        cli_cfg: &Option<RoozCfg>,
        repo_cfg: &Option<RoozCfg>,
    ) -> String {
        cli.image
            .clone()
            .or(cli_cfg.clone().map(|c| c.image).flatten())
            .or(repo_cfg.clone().map(|c| c.image).flatten())
            .or(cli.env_image.clone())
            .unwrap_or(constants::DEFAULT_IMAGE.into())
    }

    pub fn user(cli: &WorkParams, cli_cfg: &Option<RoozCfg>, repo_cfg: &Option<RoozCfg>) -> String {
        cli.user
            .clone()
            .or(cli_cfg.clone().map(|c| c.user).flatten())
            .or(repo_cfg.clone().map(|c| c.user).flatten())
            .or(cli.env_user.clone())
            .unwrap_or(constants::DEFAULT_USER.into())
    }

    pub fn sidecars(
        cli_cfg: &Option<RoozCfg>,
        repo_cfg: &Option<RoozCfg>,
    ) -> Option<HashMap<String, RoozSidecar>> {
        //TODO: test this with duplicate keys
        let mut all_sidecars = HashMap::<String, RoozSidecar>::new();

        if let Some(sidecars) = cli_cfg.clone().map(|c| c.sidecars).flatten() {
            all_sidecars.extend(sidecars);
        };

        if let Some(sidecars) = repo_cfg.clone().map(|c| c.sidecars).flatten() {
            all_sidecars.extend(sidecars);
        };
        if all_sidecars.len() > 0 {
            Some(all_sidecars)
        } else {
            None
        }
    }

    pub fn caches(
        cli: &WorkParams,
        cli_cfg: &Option<RoozCfg>,
        repo_cfg: &Option<RoozCfg>,
    ) -> Vec<String> {
        let mut all_caches = vec![];
        if let Some(caches) = cli.caches.clone() {
            all_caches.extend(caches);
        }

        if let Some(caches) = cli_cfg.clone().map(|c| c.caches).flatten() {
            all_caches.extend(caches);
        };

        if let Some(caches) = repo_cfg.clone().map(|c| c.caches).flatten() {
            all_caches.extend(caches);
        };

        if let Some(caches) = cli.env_caches.clone() {
            all_caches.extend(caches);
        }
        all_caches.dedup();
        all_caches
    }
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
    pub fn safe_volume_name(&self) -> String {
        let safe_id = to_safe_id(self.role.as_str());

        match self {
            RoozVolume {
                sharing: RoozVolumeSharing::Exclusive { key },
                ..
            } => format!("rooz-{}-{}", to_safe_id(&key), &safe_id),
            RoozVolume {
                path: p,
                sharing: RoozVolumeSharing::Shared,
                role: RoozVolumeRole::Cache,
                ..
            } => format!("rooz-{}-{}", to_safe_id(&p), &safe_id),
            RoozVolume { .. } => format!("rooz-{}", &safe_id),
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
}

#[derive(Clone, Debug)]
pub struct GitCloneSpec {
    pub dir: String,
    pub mount: Mount,
    pub volume: RoozVolume,
}

#[derive(Clone, Debug)]
pub struct WorkSpec<'a> {
    pub image: &'a str,
    pub shell: &'a str,
    pub uid: &'a str,
    pub user: &'a str,
    pub container_working_dir: &'a str,
    pub container_name: &'a str,
    pub workspace_key: &'a str,
    pub labels: HashMap<&'a str, &'a str>,
    pub ephemeral: bool,
    pub git_vol_mount: Option<Mount>,
    pub caches: Option<Vec<String>>,
    pub privileged: bool,
    pub force_recreate: bool,
    pub network: Option<&'a str>,
}

impl Default for WorkSpec<'_> {
    fn default() -> Self {
        Self {
            image: Default::default(),
            shell: Default::default(),
            uid: Default::default(),
            user: Default::default(),
            container_working_dir: Default::default(),
            container_name: Default::default(),
            workspace_key: Default::default(),
            labels: Default::default(),
            ephemeral: false,
            git_vol_mount: None,
            caches: None,
            privileged: false,
            force_recreate: false,
            network: None,
        }
    }
}

pub struct RunSpec<'a> {
    pub reason: &'a str,
    pub image: &'a str,
    pub uid: &'a str,
    pub user: &'a str,
    pub work_dir: Option<&'a str>,
    pub home_dir: &'a str,
    pub container_name: &'a str,
    pub workspace_key: &'a str,
    pub mounts: Option<Vec<Mount>>,
    pub entrypoint: Option<Vec<&'a str>>,
    pub privileged: bool,
    pub force_recreate: bool,
    pub auto_remove: bool,
    pub labels: HashMap<&'a str, &'a str>,
    pub env: Option<HashMap<String, String>>,
    pub network: Option<&'a str>,
    pub network_aliases: Option<Vec<String>>,
    pub command: Option<Vec<&'a str>>,
}

impl Default for RunSpec<'_> {
    fn default() -> Self {
        Self {
            reason: Default::default(),
            image: Default::default(),
            uid: Default::default(),
            user: Default::default(),
            work_dir: None,
            home_dir: Default::default(),
            container_name: Default::default(),
            workspace_key: Default::default(),
            mounts: None,
            entrypoint: None,
            privileged: false,
            force_recreate: false,
            auto_remove: false,
            labels: Default::default(),
            env: Default::default(),
            network: None,
            network_aliases: None,
            command: None,
        }
    }
}

pub struct WorkspaceResult {
    pub container_id: String,
    pub volumes: Vec<RoozVolume>,
}
