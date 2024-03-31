use std::{collections::HashMap, fs};

use crate::{cli::WorkParams, constants, id::to_safe_id};
use bollard::service::{Mount, MountTypeEnum};
use serde::Deserialize;

pub type AnyError = Box<dyn std::error::Error + 'static>;

#[derive(Debug, Deserialize, Clone)]
pub struct RoozSidecar {
    pub image: String,
    pub env: Option<HashMap<String, String>>,
    pub command: Option<Vec<String>>,
    pub mounts: Option<Vec<String>>,
    pub ports: Option<Vec<String>>,
    pub mount_work: Option<bool>,
    pub work_dir: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RoozCfg {
    pub shell: Option<String>,
    pub image: Option<String>,
    pub user: Option<String>,
    pub caches: Option<Vec<String>>,
    pub sidecars: Option<HashMap<String, RoozSidecar>>,
    pub env: Option<HashMap<String, String>>,
    pub ports: Option<Vec<String>>,
    pub git_ssh_url: Option<String>,
    pub privileged: Option<bool>,
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

    pub fn shell(cli_shell: &str, cli_cfg: &Option<RoozCfg>, repo_cfg: &Option<RoozCfg>) -> String {
        Some(cli_shell.into())
            .clone()
            .or(cli_cfg.clone().map(|c| c.shell).flatten())
            .or(repo_cfg.clone().map(|c| c.shell).flatten())
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

    pub fn git_ssh_url(cli: &WorkParams, cli_cfg: &Option<RoozCfg>) -> Option<String> {
        cli.git_ssh_url
            .clone()
            .or(cli_cfg.clone().map(|c| c.git_ssh_url).flatten())
            .or(cli.git_ssh_url.clone())
    }

    pub fn privileged(
        cli: &WorkParams,
        cli_cfg: &Option<RoozCfg>,
        repo_cfg: &Option<RoozCfg>,
    ) -> bool {
        cli.privileged
            .clone()
            .or(cli_cfg.clone().map(|c| c.privileged).flatten())
            .or(repo_cfg.clone().map(|c| c.privileged).flatten())
            .unwrap_or(false)
    }

    pub fn sidecars(
        cli_cfg: &Option<RoozCfg>,
        repo_cfg: &Option<RoozCfg>,
    ) -> Option<HashMap<String, RoozSidecar>> {
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

    pub fn parse_ports<'a>(
        map: &'a mut HashMap<String, String>,
        ports: Option<Vec<String>>,
    ) -> &'a HashMap<String, String> {
        match ports {
            None => map,
            Some(ports) => {
                for (source, target) in ports.iter().map(RoozCfg::parse_port) {
                    map.insert(source.to_string(), target.to_string());
                }
                map
            }
        }
    }

    fn parse_port(port_mapping: &String) -> (u16, u16) {
        match port_mapping.split(":").collect::<Vec<_>>().as_slice() {
            &[a, b] => (a.parse::<u16>().unwrap(), b.parse::<u16>().unwrap()),
            _ => panic!("Invalid port mapping specification: {}", port_mapping),
        }
    }

    pub fn ports(
        cli_cfg: &Option<RoozCfg>,
        repo_cfg: &Option<RoozCfg>,
    ) -> Option<HashMap<String, String>> {
        let mut all_ports = HashMap::<String, String>::new();

        RoozCfg::parse_ports(&mut all_ports, cli_cfg.clone().map(|c| c.ports).flatten());
        RoozCfg::parse_ports(&mut all_ports, repo_cfg.clone().map(|c| c.ports).flatten());

        if all_ports.len() > 0 {
            Some(all_ports)
        } else {
            None
        }
    }

    pub fn env_vars(
        cli_cfg: &Option<RoozCfg>,
        repo_cfg: &Option<RoozCfg>,
    ) -> Option<HashMap<String, String>> {
        let mut all_env_vars = HashMap::<String, String>::new();

        if let Some(env) = cli_cfg.clone().map(|c| c.env).flatten() {
            all_env_vars.extend(env);
        };

        if let Some(env) = repo_cfg.clone().map(|c| c.env).flatten() {
            all_env_vars.extend(env);
        };
        if all_env_vars.len() > 0 {
            Some(all_env_vars)
        } else {
            None
        }
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

pub const CACHE_ROLE: &'static str = "cache";

#[derive(Debug, Clone)]
pub enum RoozVolumeRole {
    Home,
    Work,
    Cache,
    Data,
    SshKey,
}

impl RoozVolumeRole {
    pub fn as_str(&self) -> &str {
        match self {
            RoozVolumeRole::Home => "home",
            RoozVolumeRole::Work => "work",
            RoozVolumeRole::Cache => CACHE_ROLE,
            RoozVolumeRole::Data => "data",
            RoozVolumeRole::SshKey => "ssh-key",
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

    pub fn to_mount(&self, tilde_replacement: Option<&str>) -> Mount {
        let vol_name = self.safe_volume_name();

        let modified_path = match tilde_replacement {
            Some(replacement) => self.path.replace("~", &replacement),
            None => self.path.to_string(),
        };

        Mount {
            typ: Some(MountTypeEnum::VOLUME),
            source: Some(vol_name.into()),
            target: Some(modified_path),
            read_only: Some(false),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitCloneSpec {
    pub dir: String,
}

#[derive(Clone, Debug)]
pub struct WorkSpec<'a> {
    pub image: &'a str,
    pub uid: &'a str,
    pub user: &'a str,
    pub container_working_dir: &'a str,
    pub container_name: &'a str,
    pub workspace_key: &'a str,
    pub labels: HashMap<&'a str, &'a str>,
    pub ephemeral: bool,
    pub caches: Option<Vec<String>>,
    pub privileged: bool,
    pub force_recreate: bool,
    pub network: Option<&'a str>,
    pub env_vars: Option<HashMap<String, String>>,
    pub ports: Option<HashMap<String, String>>,
}

impl Default for WorkSpec<'_> {
    fn default() -> Self {
        Self {
            image: Default::default(),
            uid: Default::default(),
            user: Default::default(),
            container_working_dir: Default::default(),
            container_name: Default::default(),
            workspace_key: Default::default(),
            labels: Default::default(),
            ephemeral: false,
            caches: None,
            privileged: false,
            force_recreate: false,
            network: None,
            env_vars: None,
            ports: None,
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
    pub ports: Option<HashMap<String, String>>,
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
            ports: None,
        }
    }
}

pub struct WorkspaceResult {
    pub container_id: String,
    pub volumes: Vec<RoozVolume>,
    pub workspace_key: String,
    pub working_dir: String,
    pub home_dir: String,
    pub orig_uid: String,
}

pub struct EnterSpec {
    pub workspace: WorkspaceResult,
    pub git_spec: Option<GitCloneSpec>,
    pub git_repo_config: Option<RoozCfg>,
}
