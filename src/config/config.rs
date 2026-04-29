use crate::model::types::ContentGenerator::{Inline, Script};
use crate::model::types::{AnyError, ContentGenerator, DataEntryKey};
use crate::util::id;
use crate::{cli::WorkParams, constants};
use colored::Colorize;
use handlebars::{Handlebars, no_escape};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

#[derive(Debug, Clone)]
pub enum ConfigSource {
    Update {
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
        if value.contains(":") {
            let chunks = value.split("//").collect::<Vec<_>>();
            match chunks.as_slice() {
                &[url, file_path] => Ok(Self::Git {
                    url: url.to_string(),
                    file_path: file_path.to_string(),
                }),
                _ => Err(format!("Invalid remote config spec URL {}", value).into()),
            }
        } else {
            Ok(Self::File {
                path: value.to_string(),
            })
        }
    }

    pub fn is_in_repo(&self) -> bool {
        if let ConfigPath::Git { file_path, .. } = self {
            file_path == ".rooz.yaml"
        } else {
            false
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
pub enum ConfigType {
    Body,
    Runtime,
}

impl ConfigType {
    pub fn file_path(&self) -> &str {
        match self {
            ConfigType::Body => "workspace.config",
            ConfigType::Runtime => "runtime.config",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FileFormat {
    Yaml,
}

impl FileFormat {
    pub fn to_string(&self) -> String {
        match self {
            FileFormat::Yaml => "yaml".into(),
        }
    }

    pub fn from_path(path: &str) -> FileFormat {
        match Path::new(path).extension().and_then(OsStr::to_str) {
            Some("yaml") => FileFormat::Yaml,
            Some(other) => panic!("Config file format: {} is not supported", other),
            None => panic!("Only yaml config file format is supported."),
        }
    }
}

#[serde_with::skip_serializing_none]
#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RoozSidecar {
    pub image: String,
    pub env: Option<IndexMap<String, String>>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub mounts: Option<IndexMap<String, MountSource>>,
    pub ports: Option<Vec<String>>,
    pub privileged: Option<bool>,
    pub init: Option<bool>,
    pub install: Option<String>,
    pub work_dir: Option<String>,
    pub user: Option<String>,
    pub uid: Option<i32>,
    pub egress: Option<bool>,
    pub shell: Option<Vec<String>>,
}

impl RoozSidecar {
    pub fn expand_vars(
        &self,
        reg: &Handlebars,
        vars: &IndexMap<String, String>,
    ) -> Result<Self, AnyError> {
        Ok(Self {
            image: render_str(reg, &self.image, vars)?,
            env: render_map(reg, &self.env, vars)?,
            command: render_vec(reg, &self.command, vars)?,
            args: render_vec(reg, &self.args, vars)?,
            shell: render_vec(reg, &self.shell, vars)?,
            ports: render_vec(reg, &self.ports, vars)?,
            install: render_opt(reg, &self.install, vars)?,
            work_dir: render_opt(reg, &self.work_dir, vars)?,
            user: render_opt(reg, &self.user, vars)?,
            mounts: self
                .mounts
                .as_ref()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| Ok((k.clone(), v.expand_vars(reg, vars)?)))
                        .collect::<Result<IndexMap<_, _>, AnyError>>()
                })
                .transpose()?,
            privileged: self.privileged,
            init: self.init,
            uid: self.uid,
            egress: self.egress,
        })
    }
}

#[serde_with::skip_serializing_none]
#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RoozCfg {
    pub extends: Option<String>,
    pub vars: Option<IndexMap<String, String>>,
    pub secrets: Option<IndexMap<String, String>>,
    pub git_ssh_url: Option<String>,
    pub extra_repos: Option<Vec<String>>,
    pub image: Option<String>,
    pub caches: Option<Vec<String>>,
    pub shell: Option<Vec<String>>,
    pub user: Option<String>,
    pub ports: Option<Vec<String>>,
    pub privileged: Option<bool>,
    pub init: Option<bool>,
    pub install: Option<String>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub env: Option<IndexMap<String, String>>,
    pub sidecars: Option<IndexMap<String, RoozSidecar>>,
    pub data: Option<IndexMap<String, DataValue>>,
    pub mounts: Option<IndexMap<String, MountSource>>,
}

impl Default for RoozCfg {
    fn default() -> Self {
        Self {
            extends: None,
            vars: Some(IndexMap::new()),
            secrets: Some(IndexMap::new()),
            git_ssh_url: None,
            extra_repos: Some(Vec::new()),
            image: Some(constants::DEFAULT_IMAGE.into()),
            caches: Some(Vec::new()),
            shell: Some(vec![constants::DEFAULT_SHELL.into()]),
            user: Some(constants::DEFAULT_USER.into()),
            ports: Some(Vec::new()),
            privileged: None,
            init: None,
            command: Some(Vec::new()),
            args: Some(Vec::new()),
            env: Some(IndexMap::new()),
            sidecars: Some(IndexMap::new()),
            data: Some(IndexMap::new()),
            mounts: Some(IndexMap::new()),
            install: None,
        }
    }
}

impl RoozCfg {
    pub fn from_string(config: &str, file_format: FileFormat) -> Result<Self, AnyError> {
        Ok(match file_format {
            FileFormat::Yaml => serde_yaml::from_str(&config)?,
        })
    }

    pub fn to_string(&self, file_format: FileFormat) -> Result<String, AnyError> {
        Ok(match file_format {
            FileFormat::Yaml => serde_yaml::to_string(&self)?,
        })
    }

    fn extend_if_any<A, T: Extend<A> + IntoIterator<Item = A>>(
        target: Option<T>,
        other: Option<T>,
    ) -> Option<T> {
        match (target, other) {
            (Some(mut t), Some(o)) => {
                t.extend(o);
                Some(t)
            }
            (t, None) => t,
            (None, o) => o,
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
            extends: None,
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
            init: config.init.clone().or(self.init.clone()),
            command: Self::extend_if_any(self.command.clone(), config.command.clone()),
            args: Self::extend_if_any(self.args.clone(), config.args.clone()),
            env: Self::extend_if_any(self.env.clone(), config.env.clone()),
            sidecars: Self::extend_if_any(self.sidecars.clone(), config.sidecars.clone()),
            data: Self::extend_if_any(self.data.clone(), config.data.clone()),
            mounts: Self::extend_if_any(self.mounts.clone(), config.mounts.clone()),
            install: config.install.clone().or(self.install.clone()),
        }
    }

    pub fn validate_extends_path(path: &str) -> Result<(), AnyError> {
        if path.contains(':') {
            return Err(format!("extends path must be a local relative path (no URLs): '{}'", path).into());
        }
        if path.starts_with('/') {
            return Err(format!("extends path must be relative, not absolute: '{}'", path).into());
        }
        Ok(())
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
        ports: Vec<String>,
    ) -> &'a HashMap<String, Option<String>> {
        match ports.as_slice() {
            &[] => map,
            ports => {
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
            (None, None) => IndexMap::<String, String>::new(),
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

        let mut reg = Handlebars::new();
        reg.register_escape_fn(no_escape);
        let mut built_vars = IndexMap::<String, String>::new();

        for (k, v) in vars_and_secrets {
            built_vars.insert(k.to_string(), reg.render_template(&v, &built_vars)?);
        }

        self.vars = render_map(&reg, &self.vars, &built_vars)?;
        self.git_ssh_url = render_opt(&reg, &self.git_ssh_url, &built_vars)?;
        self.image = render_opt(&reg, &self.image, &built_vars)?;
        self.user = render_opt(&reg, &self.user, &built_vars)?;
        self.install = render_opt(&reg, &self.install, &built_vars)?;
        self.shell = render_vec(&reg, &self.shell, &built_vars)?;
        self.command = render_vec(&reg, &self.command, &built_vars)?;
        self.args = render_vec(&reg, &self.args, &built_vars)?;
        self.caches = render_vec(&reg, &self.caches, &built_vars)?;
        self.ports = render_vec(&reg, &self.ports, &built_vars)?;
        self.extra_repos = render_vec(&reg, &self.extra_repos, &built_vars)?;
        self.env = render_map(&reg, &self.env, &built_vars)?;
        self.sidecars = self
            .sidecars
            .as_ref()
            .map(|m| {
                m.iter()
                    .map(|(k, s)| Ok((k.clone(), s.expand_vars(&reg, &built_vars)?)))
                    .collect::<Result<IndexMap<_, _>, AnyError>>()
            })
            .transpose()?;
        self.data = self
            .data
            .as_ref()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| Ok((k.clone(), v.expand_vars(&reg, &built_vars)?)))
                    .collect::<Result<IndexMap<_, _>, AnyError>>()
            })
            .transpose()?;
        self.mounts = self
            .mounts
            .as_ref()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| Ok((k.clone(), v.expand_vars(&reg, &built_vars)?)))
                    .collect::<Result<IndexMap<_, _>, AnyError>>()
            })
            .transpose()?;

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
fn render_str(
    reg: &Handlebars,
    val: &str,
    vars: &IndexMap<String, String>,
) -> Result<String, AnyError> {
    Ok(reg.render_template(val, vars)?)
}

fn render_opt(
    reg: &Handlebars,
    val: &Option<String>,
    vars: &IndexMap<String, String>,
) -> Result<Option<String>, AnyError> {
    val.as_ref().map(|s| render_str(reg, s, vars)).transpose()
}

fn render_map(
    reg: &Handlebars,
    val: &Option<IndexMap<String, String>>,
    vars: &IndexMap<String, String>,
) -> Result<Option<IndexMap<String, String>>, AnyError> {
    val.as_ref()
        .map(|m| {
            m.iter()
                .map(|(k, v)| Ok((k.clone(), render_str(reg, v, vars)?)))
                .collect()
        })
        .transpose()
}

fn render_vec(
    reg: &Handlebars,
    val: &Option<Vec<String>>,
    vars: &IndexMap<String, String>,
) -> Result<Option<Vec<String>>, AnyError> {
    val.as_ref()
        .map(|v| v.iter().map(|s| render_str(reg, s, vars)).collect())
        .transpose()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SystemConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_key: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub gitconfig: Option<String>,
}

impl SystemConfig {
    pub fn from_string(config: &str) -> Result<Self, AnyError> {
        Ok(serde_yaml::from_str(&config)?)
    }

    pub fn to_string(config: &Self) -> Result<String, AnyError> {
        Ok(serde_yaml::to_string(&config)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum DataValue {
    Dir {},
    InlineContent {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        executable: Option<bool>,
    },
    GeneratedContent {
        generate: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        image: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        executable: Option<bool>,
    },
}

impl DataValue {
    pub fn into_entry(self, name: String) -> DataEntry {
        match self {
            DataValue::InlineContent {
                content,
                executable,
            } => DataEntry::File {
                name,
                generator: Inline(content),
                executable: executable.unwrap_or_default(),
            },
            DataValue::GeneratedContent {
                generate,
                image,
                executable,
            } => DataEntry::File {
                name,
                generator: Script {
                    script: generate,
                    image,
                },
                executable: executable.unwrap_or_default(),
            },
            DataValue::Dir {} => DataEntry::Dir { name },
        }
    }

    pub fn expand_vars(
        &self,
        reg: &Handlebars,
        vars: &IndexMap<String, String>,
    ) -> Result<Self, AnyError> {
        Ok(match self {
            DataValue::Dir {} => DataValue::Dir {},
            DataValue::InlineContent {
                content,
                executable,
            } => DataValue::InlineContent {
                content: render_str(reg, content, vars)?,
                executable: *executable,
            },
            DataValue::GeneratedContent {
                generate,
                image,
                executable,
            } => DataValue::GeneratedContent {
                generate: render_str(reg, generate, vars)?,
                image: render_opt(reg, image, vars)?,
                executable: *executable,
            },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum MountSource {
    DataEntryReference(DataEntryKey),
    InlineDataValue(DataValue),
}

impl MountSource {
    pub fn resolve_key(&self, target: &str) -> String {
        match self {
            MountSource::DataEntryReference(data_key) => data_key.as_str().to_string(),
            MountSource::InlineDataValue(_) => id::sanitize(target),
        }
    }

    pub fn expand_vars(
        &self,
        reg: &Handlebars,
        vars: &IndexMap<String, String>,
    ) -> Result<Self, AnyError> {
        Ok(match self {
            MountSource::DataEntryReference(k) => MountSource::DataEntryReference(k.clone()),
            MountSource::InlineDataValue(dv) => {
                MountSource::InlineDataValue(dv.expand_vars(reg, vars)?)
            }
        })
    }
}

#[derive(Debug, Clone)]
pub enum DataEntry {
    Dir {
        name: String,
    },
    File {
        name: String,
        generator: ContentGenerator,
        executable: bool,
    },
}

impl DataEntry {
    pub fn name(self) -> String {
        match self {
            DataEntry::Dir { name } => name,
            DataEntry::File { name, .. } => name,
        }
    }
}

pub trait DataExt {
    fn into_entries(self) -> Vec<DataEntry>;
}

impl<T> DataExt for T
where
    T: IntoIterator<Item = (String, DataValue)>,
{
    fn into_entries(self) -> Vec<DataEntry> {
        self.into_iter()
            .map(|(name, value)| value.into_entry(name))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all_variants() {
        let yaml = r#"
data:
  some-dir: {}

  inline-file:
    content: |
      some content
      here

  empty-file:
    content: ""

  generated-file:
    generate: |
      echo -n "new content"


"#;

        let config: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        let entries = config.data.unwrap().into_entries();

        assert_eq!(entries.len(), 4);

        for entry in &entries {
            match entry {
                DataEntry::Dir { name } => {
                    assert_eq!(name, "some-dir");
                }
                DataEntry::File {
                    name,
                    generator: Inline(_),
                    ..
                } => {
                    assert!(name == "inline-file" || name == "empty-file");
                }
                DataEntry::File {
                    name,
                    generator: Script { script: _, .. },
                    ..
                } => {
                    assert_eq!(name, "generated-file");
                }
            }
        }
    }
}
