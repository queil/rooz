use super::types::AnyError;
use crate::{cli::WorkParams, constants};
use colored::Colorize;
use handlebars::{no_escape, Handlebars};
use linked_hash_map::LinkedHashMap;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, ffi::OsStr, fs, path::Path};

#[derive(Debug, Clone)]
pub enum ConfigSource {
    Body {
        value: RoozCfg,
        origin: String,
        format: FileFormat,
    },
    Path {
        value: ConfigPath,
    },
}

#[derive(Debug, Clone)]
pub enum ConfigPath {
    File { path: String },
    Git { url: String, file_path: String },
}

impl<'a> ConfigPath {
    pub fn from_str(value: &'a str) -> Result<Self, AnyError> {
        if value.starts_with("git@") || value.starts_with("ssh://") {
            let chunks = value.split("//").collect::<Vec<_>>();
            match chunks.as_slice() {
                &[url, file_path] => Ok(Self::Git {
                    url: url.to_string(),
                    file_path: file_path.to_string(),
                }),
                _ => Err(format!("Invalid repo URL {}", value).into()),
            }
        } else {
            Ok(Self::File {
                path: value.to_string(),
            })
        }
    }

    pub fn to_string(&self) -> String {
        match self {
            ConfigPath::File { path } => path.to_string(),
            ConfigPath::Git { url, file_path } => format!("{}//{}", url, file_path),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FileFormat {
    Toml,
    Yaml,
}

impl FileFormat {
    pub fn to_string(&self) -> String {
        match self {
            FileFormat::Toml => "toml".into(),
            FileFormat::Yaml => "yaml".into(),
        }
    }

    pub fn from_path(path: &str) -> FileFormat {
        match Path::new(path).extension().and_then(OsStr::to_str) {
            Some("yaml") => FileFormat::Yaml,
            Some("toml") => FileFormat::Toml,
            Some(other) => panic!("Config file format: {} is not supported", other),
            None => panic!("Only toml and yaml config file formats are supported."),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RoozSidecar {
    pub image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<LinkedHashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mounts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount_work: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RoozCfg {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vars: Option<LinkedHashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets: Option<LinkedHashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_ssh_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_repos: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caches: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privileged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<LinkedHashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidecars: Option<LinkedHashMap<String, RoozSidecar>>,
}

impl Default for RoozCfg {
    fn default() -> Self {
        Self {
            vars: Some(LinkedHashMap::new()),
            secrets: Some(LinkedHashMap::new()),
            git_ssh_url: None,
            extra_repos: Some(Vec::new()),
            image: Some(constants::DEFAULT_IMAGE.into()),
            caches: Some(Vec::new()),
            shell: Some(vec![constants::DEFAULT_SHELL.into()]),
            user: Some(constants::DEFAULT_USER.into()),
            ports: Some(Vec::new()),
            privileged: None,
            env: Some(LinkedHashMap::new()),
            sidecars: Some(LinkedHashMap::new()),
        }
    }
}

impl RoozCfg {
    pub fn from_string(config: &str, file_format: FileFormat) -> Result<Self, AnyError> {
        Ok(match file_format {
            FileFormat::Yaml => serde_yaml::from_str(&config)?,
            FileFormat::Toml => toml::from_str(&config)?,
        })
    }

    pub fn to_file(&self, path: &str) -> Result<(), AnyError> {
        let file_format = FileFormat::from_path(path);
        fs::write(path, self.to_string(file_format)?)?;
        Ok(())
    }

    pub fn to_string(&self, file_format: FileFormat) -> Result<String, AnyError> {
        Ok(match file_format {
            FileFormat::Yaml => serde_yaml::to_string(&self)?,
            FileFormat::Toml => toml::to_string(&self)?,
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

    pub fn from_cli(&mut self, cli: &WorkParams, shell: Option<String>) -> () {
        *self = RoozCfg {
            shell: shell.map(|v| vec![v]).or(self.shell.clone()),
            image: cli.image.clone().or(self.image.clone()),
            user: cli.user.clone().or(self.user.clone()),
            git_ssh_url: cli.git_ssh_url.clone().or(self.git_ssh_url.clone()),
            privileged: cli.privileged.or(self.privileged),
            caches: Self::extend_if_any(self.caches.clone(), cli.caches.clone()),
            ..self.clone()
        }
    }

    pub fn from_config(&mut self, config: &RoozCfg) -> () {
        *self = RoozCfg {
            vars: Self::extend_if_any(self.vars.clone(), config.vars.clone()),
            secrets: Self::extend_if_any(self.secrets.clone(), config.secrets.clone()),
            git_ssh_url: config.git_ssh_url.clone().or(self.git_ssh_url.clone()),
            extra_repos: Self::extend_if_any(self.extra_repos.clone(), config.extra_repos.clone()),
            image: config.image.clone().or(self.image.clone()),
            caches: Self::extend_if_any(self.caches.clone(), config.caches.clone()),
            shell: config.shell.clone().or(self.shell.clone()),
            user: config.user.clone().or(self.user.clone()),
            ports: Self::extend_if_any(self.ports.clone(), config.ports.clone()),
            privileged: config.privileged.clone().or(self.privileged.clone()),
            env: Self::extend_if_any(self.env.clone(), config.env.clone()),
            sidecars: Self::extend_if_any(self.sidecars.clone(), config.sidecars.clone()),
        }
    }

    pub fn from_cli_env(self, cli: WorkParams) -> Self {
        RoozCfg {
            shell: cli.env.shell.map(|v| vec![v]).or(self.shell.clone()),
            image: cli.env.image.or(self.image.clone()),
            user: cli.env.user.or(self.user.clone()),
            caches: Self::extend_if_any(self.caches.clone(), cli.env.caches),
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
        map: &'a mut HashMap<String, Option<String>>,
        ports: Option<Vec<String>>,
    ) -> &'a HashMap<String, Option<String>> {
        match ports {
            None => map,
            Some(ports) => {
                for (source, target) in ports.iter().map(RoozCfg::parse_port) {
                    map.insert(source.to_string(), target.map(|p| p.to_string()));
                }
                map
            }
        }
    }

    fn parse_port(port_mapping: &String) -> (u16, Option<u16>) {
        match port_mapping.split(":").collect::<Vec<_>>().as_slice() {
            &[a] => (a.parse::<u16>().unwrap(), None),
            &[a, b] => (a.parse::<u16>().unwrap(), Some(b.parse::<u16>().unwrap())),
            _ => panic!("Invalid port mapping specification: {}", port_mapping),
        }
    }

    pub fn expand_vars(&mut self) -> Result<(), AnyError> {
        let vars_and_secrets = match (&self.vars, &self.secrets) {
            (None, None) => LinkedHashMap::<String, String>::new(),
            (None, Some(secrets)) => secrets.clone(),
            (Some(vars), None) => vars.clone(),
            (Some(vars), Some(secrets)) => {
                if let Some(duplicate_key) =
                    vars.keys().find(|k| secrets.contains_key(&k.to_string()))
                {
                    panic!(
                        "The key: '{}' can be only defined in either vars or secrets.",
                        &duplicate_key.to_string()
                    )
                }

                let mut secrets = secrets.clone();
                secrets.extend(vars.clone());
                secrets
            }
        };

        let cfg_string = &self.to_string(FileFormat::Yaml)?;
        let mut reg = Handlebars::new();
        reg.register_escape_fn(no_escape);
        let mut built_vars = LinkedHashMap::<String, String>::new();

        for (k, v) in vars_and_secrets {
            built_vars.insert(k.to_string(), reg.render_template(&v, &built_vars)?);
        }

        let rendered = reg.render_template(&cfg_string, &built_vars)?;
        let s = RoozCfg::from_string(&rendered, FileFormat::Yaml)?;
        *self = s;

        Ok(())
    }

    pub fn deserialize_config(
        config: &str,
        file_format: FileFormat,
    ) -> Result<Option<RoozCfg>, AnyError> {
        match RoozCfg::from_string(config, file_format) {
            Ok(cfg) => Ok(Some(cfg)),
            Err(e) => {
                eprintln!(
                    "{}\n{}",
                    format!(
                        "WARNING: Could not read config ({})",
                        file_format.to_string()
                    )
                    .bold()
                    .yellow(),
                    e.to_string().yellow()
                );
                Ok(None)
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FinalCfg {
    pub git_ssh_url: Option<String>,
    pub extra_repos: Vec<String>,
    pub image: String,
    pub caches: Vec<String>,
    pub shell: Vec<String>,
    pub user: String,
    pub ports: HashMap<String, Option<String>>,
    pub privileged: bool,
    pub env: HashMap<String, String>,
    pub sidecars: HashMap<String, RoozSidecar>,
}

impl Default for FinalCfg {
    fn default() -> Self {
        Self {
            git_ssh_url: None,
            extra_repos: Vec::new(),
            image: constants::DEFAULT_IMAGE.into(),
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

impl FinalCfg {
    pub fn from_string(config: String) -> Result<FinalCfg, Box<dyn std::error::Error + 'static>> {
        match toml::from_str(&config) {
            Ok(val) => Ok(val),
            Err(e) => Err(Box::new(e)),
        }
    }

    pub fn to_string(&self) -> Result<String, AnyError> {
        match toml::to_string(&self) {
            Ok(val) => Ok(val),
            Err(e) => Err(Box::new(e)),
        }
    }
}

impl<'a> From<&'a RoozCfg> for FinalCfg {
    fn from(value: &'a RoozCfg) -> Self {
        let default = FinalCfg::default();

        let mut ports = HashMap::<String, Option<String>>::new();
        RoozCfg::parse_ports(&mut ports, value.clone().ports);

        FinalCfg {
            git_ssh_url: value.git_ssh_url.clone(),
            extra_repos: value
                .extra_repos
                .as_deref()
                .unwrap_or(&default.extra_repos)
                .to_vec(),
            shell: value.shell.as_deref().unwrap_or(&default.shell).into(),
            image: value.image.as_deref().unwrap_or(&default.image).into(),
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
