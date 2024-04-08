use crate::{cli::WorkParams, constants};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, ffi::OsStr, fs, path::Path};

use super::types::AnyError;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RoozSidecar {
    pub image: String,
    pub env: Option<HashMap<String, String>>,
    pub command: Option<Vec<String>>,
    pub mounts: Option<Vec<String>>,
    pub ports: Option<Vec<String>>,
    pub mount_work: Option<bool>,
    pub work_dir: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RoozCfg {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caches: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidecars: Option<HashMap<String, RoozSidecar>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_ssh_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privileged: Option<bool>,
}

#[derive(Debug, Clone)]
pub enum FileFormat {
    Toml,
    Yaml,
}

impl FileFormat {
    pub fn to_string(&self) -> String {
        match self {
            FileFormat::Toml => "toml".into(),
            FileFormat::Yaml => "y*ml".into(),
        }
    }

    pub fn from_path(path: &str) -> FileFormat {
        match Path::new(path).extension().and_then(OsStr::to_str) {
            Some("yaml") => FileFormat::Yaml,
            Some("yml") => FileFormat::Yaml,
            Some("toml") => FileFormat::Toml,
            Some(other) => panic!("Config file format: {} is not supported", other),
            None => panic!("Only toml and yaml config file formats are supported."),
        }
    }
}

impl RoozCfg {
    pub fn from_file(path: &str) -> Result<Self, AnyError> {
        Self::from_string(fs::read_to_string(path)?, FileFormat::from_path(&path))
    }

    pub fn from_string(config: String, file_format: FileFormat) -> Result<Self, AnyError> {
        Ok(match file_format {
            FileFormat::Yaml => Self::deserialize(serde_yaml::Deserializer::from_str(&config))?,
            FileFormat::Toml => Self::deserialize(toml::de::Deserializer::new(&config))?,
        })
    }

    pub fn to_file(&self, path: &str) -> Result<(), AnyError> {
        let file_format = FileFormat::from_path(path);
        fs::write(path, self.to_string(file_format)?)?;
        Ok(())
    }

    pub fn to_string(&self, file_format: FileFormat) -> Result<String, AnyError> {
        Ok(match file_format {
            FileFormat::Yaml => {
                let mut ret = Vec::new();
                let mut ser = serde_yaml::Serializer::new(&mut ret);
                self.serialize(&mut ser)?;
                std::str::from_utf8(&ret)?.to_string()
            }
            FileFormat::Toml => {
                let mut ret = String::new();
                Self::serialize(&self, toml::ser::Serializer::new(&mut ret))?;
                ret
            }
        })
    }

    fn extend_if_any<A, T: Extend<A> + IntoIterator<Item = A>>(
        target: Option<T>,
        other: Option<T>,
    ) -> Option<T> {
        if let Some(caches) = other {
            let mut ret = target.unwrap();
            ret.extend(caches);
            Some(ret)
        } else {
            target
        }
    }

    pub fn from_cli(self, cli: WorkParams, shell: Option<String>) -> Self {
        RoozCfg {
            shell: shell.or(self.shell.clone()),
            image: cli.image.or(self.image.clone()),
            user: cli.user.or(self.user.clone()),
            git_ssh_url: cli.git_ssh_url.or(self.git_ssh_url.clone()),
            privileged: cli.privileged.or(self.privileged.clone()),
            caches: Self::extend_if_any(self.caches.clone(), cli.caches),
            ..self.clone()
        }
    }

    pub fn from_config(self, config: RoozCfg) -> Self {
        RoozCfg {
            shell: config.shell.or(self.shell),
            image: config.image.or(self.image),
            user: config.user.or(self.user),
            git_ssh_url: config.git_ssh_url.or(self.git_ssh_url),
            privileged: config.privileged.or(self.privileged),
            caches: Self::extend_if_any(self.caches, config.caches),
            sidecars: Self::extend_if_any(self.sidecars, config.sidecars),
            ports: Self::extend_if_any(self.ports, config.ports),
            env: Self::extend_if_any(self.env, config.env),
        }
    }

    pub fn from_cli_env(self, cli: WorkParams) -> Self {
        RoozCfg {
            shell: cli.env_shell.or(self.shell.clone()),
            image: cli.env_image.or(self.image.clone()),
            user: cli.env_user.or(self.user.clone()),
            caches: Self::extend_if_any(self.caches.clone(), cli.env_caches),
            git_ssh_url: cli.git_ssh_url.or(self.git_ssh_url.clone()),
            ..self.clone()
        }
    }

    pub fn git_ssh_url(cli: &WorkParams, cli_cfg: &Option<RoozCfg>) -> Option<String> {
        cli.git_ssh_url
            .clone()
            .or(cli_cfg.clone().map(|c| c.git_ssh_url).flatten())
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
}

impl Default for RoozCfg {
    fn default() -> Self {
        Self {
            shell: Some(constants::DEFAULT_SHELL.into()),
            image: Some(constants::DEFAULT_IMAGE.into()),
            user: Some(constants::DEFAULT_USER.into()),
            caches: Some(Vec::new()),
            sidecars: Some(HashMap::new()),
            env: Some(HashMap::new()),
            ports: Some(Vec::new()),
            git_ssh_url: None,
            privileged: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FinalCfg {
    pub shell: String,
    pub image: String,
    pub user: String,
    pub caches: Vec<String>,
    pub sidecars: HashMap<String, RoozSidecar>,
    pub env: HashMap<String, String>,
    pub ports: HashMap<String, String>,
    pub git_ssh_url: Option<String>,
    pub privileged: bool,
}

impl FinalCfg {
    pub fn from_string(config: String) -> Result<FinalCfg, Box<dyn std::error::Error + 'static>> {
        let f = Self::deserialize(toml::de::Deserializer::new(&config));
        match f {
            Ok(val) => Ok(val),
            Err(e) => Err(Box::new(e)),
        }
    }

    pub fn to_string(&self) -> Result<String, AnyError> {
        let mut ret = String::new();
        Self::serialize(&self, toml::ser::Serializer::new(&mut ret))?;
        Ok(ret)
    }
}

impl Default for FinalCfg {
    fn default() -> Self {
        Self {
            shell: constants::DEFAULT_SHELL.into(),
            image: constants::DEFAULT_IMAGE.into(),
            user: constants::DEFAULT_USER.into(),
            caches: Vec::new(),
            sidecars: HashMap::new(),
            env: HashMap::new(),
            ports: HashMap::new(),
            git_ssh_url: None,
            privileged: false,
        }
    }
}

impl<'a> From<&'a RoozCfg> for FinalCfg {
    fn from(value: &'a RoozCfg) -> Self {
        let default = FinalCfg::default();

        let mut ports = HashMap::<String, String>::new();
        RoozCfg::parse_ports(&mut ports, value.clone().ports);

        FinalCfg {
            shell: value.shell.as_deref().unwrap_or(&default.shell).into(),
            image: value.image.as_deref().unwrap_or(&default.image).into(),
            user: value.user.as_deref().unwrap_or(&default.user).into(),
            caches: {
                let mut val = value.caches.as_deref().unwrap_or(&default.caches).to_vec();
                val.dedup();
                val
            },
            sidecars: value.sidecars.as_ref().unwrap().clone(),
            env: value.env.as_ref().unwrap().clone(),
            ports,
            git_ssh_url: value.git_ssh_url.clone(),
            privileged: value.privileged.unwrap_or(default.privileged),
            ..default
        }
    }
}
