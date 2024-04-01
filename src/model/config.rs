use std::{collections::HashMap, fs};

use serde::Deserialize;

use crate::{cli::WorkParams, constants};

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
