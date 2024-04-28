use crate::{
    git::RootRepoCloneResult,
    labels::Labels,
    model::{config::RoozCfg, volume::RoozVolume},
};
use bollard::service::Mount;
use std::collections::HashMap;

pub type AnyError = Box<dyn std::error::Error + 'static>;

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

#[derive(Clone, Debug)]
pub struct WorkSpec<'a> {
    pub image: &'a str,
    pub uid: &'a str,
    pub user: &'a str,
    pub container_working_dir: &'a str,
    pub container_name: &'a str,
    pub workspace_key: &'a str,
    pub labels: Labels,
    pub ephemeral: bool,
    pub caches: Option<Vec<String>>,
    pub privileged: bool,
    pub force_recreate: bool,
    pub network: Option<&'a str>,
    pub env_vars: Option<HashMap<String, String>>,
    pub ports: Option<HashMap<String, Option<String>>>,
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
            labels: Labels::default(),
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
    pub labels: Labels,
    pub env: Option<HashMap<String, String>>,
    pub ports: Option<HashMap<String, Option<String>>>,
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
    pub volumes: Vec<RoozVolume>,
    pub workspace_key: String,
    pub working_dir: String,
    pub orig_uid: String,
}

pub struct EnterSpec {
    pub workspace: WorkspaceResult,
    pub git_spec: Option<RootRepoCloneResult>,
    pub config: RoozCfg,
}
