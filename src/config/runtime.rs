use super::config::{RoozCfg, RoozSidecar};
use crate::constants;
use crate::AnyError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RuntimeConfig {
    pub git_ssh_url: Option<String>,
    pub extra_repos: Vec<String>,
    pub image: String,
    pub home_from_image: Option<String>,
    pub caches: Vec<String>,
    pub shell: Vec<String>,
    pub user: String,
    pub ports: HashMap<String, Option<String>>,
    pub privileged: bool,
    pub env: HashMap<String, String>,
    pub sidecars: HashMap<String, RoozSidecar>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            git_ssh_url: None,
            extra_repos: Vec::new(),
            image: constants::DEFAULT_IMAGE.into(),
            home_from_image: None,
            caches: Vec::new(),
            shell: vec![constants::DEFAULT_SHELL.into()],
            user: constants::DEFAULT_USER.into(),
            ports: HashMap::new(),
            privileged: false,
            sidecars: HashMap::new(),
            env: HashMap::new(),
        }
    }
}

impl RuntimeConfig {
    pub fn from_string(
        config: String,
    ) -> Result<RuntimeConfig, Box<dyn std::error::Error + 'static>> {
        match serde_yaml::from_str(&config) {
            Ok(val) => Ok(val),
            Err(e) => Err(Box::new(e)),
        }
    }

    pub fn to_string(&self) -> Result<String, AnyError> {
        match serde_yaml::to_string(&self) {
            Ok(val) => Ok(val),
            Err(e) => Err(Box::new(e)),
        }
    }
}

impl<'a> From<&'a RoozCfg> for RuntimeConfig {
    fn from(value: &'a RoozCfg) -> Self {
        let default = RuntimeConfig::default();

        let mut ports = HashMap::<String, Option<String>>::new();
        RoozCfg::parse_ports(&mut ports, value.clone().ports);

        RuntimeConfig {
            git_ssh_url: value.git_ssh_url.clone(),
            extra_repos: value
                .extra_repos
                .as_deref()
                .unwrap_or(&default.extra_repos)
                .to_vec(),
            shell: value.shell.as_deref().unwrap_or(&default.shell).into(),
            image: value.image.as_deref().unwrap_or(&default.image).into(),
            home_from_image: value.home_from_image.clone(),
            user: value.user.as_deref().unwrap_or(&default.user).into(),
            caches: {
                let mut val = value.caches.as_deref().unwrap_or(&default.caches).to_vec();
                val.dedup();
                val
            },
            sidecars: value
                .sidecars
                .as_ref()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<HashMap<_, _>>(),
            env: value
                .env
                .as_ref()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<HashMap<_, _>>(),
            ports,
            privileged: value.privileged.unwrap_or(default.privileged),
            ..default
        }
    }
}
