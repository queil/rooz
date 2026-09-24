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

pub const SECRETS_NOT_ALLOWED: &str = "Refusing to expand 'secrets': this workspace merges configuration the operator did not \
     author, which could reference any of your secrets and read the plaintext from inside a \
     container it also controls. Secrets are only expanded when the whole configuration comes \
     from your own local '--config' file - not from the repository being opened (its in-repo \
     config, or a '--config git:...' source), and not from a 'bases' layer, which is a separate \
     file that may have come from anywhere. Move the settings you need into your own '--config' \
     file (inlining any base layers), or remove 'secrets' from this workspace's configuration.";

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
                &[url, file_path] => {
                    // reject the fragment here rather than at the sink: it reaches a shell
                    // in the clone container, and FileFormat::from_path would otherwise
                    // panic on it first
                    validate_relative_config_path(file_path, "config path")?;
                    Ok(Self::Git {
                        url: url.to_string(),
                        file_path: file_path.to_string(),
                    })
                }
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
    Bases,
    Runtime,
}

impl ConfigType {
    pub fn file_path(&self) -> &str {
        match self {
            ConfigType::Body => "workspace.config",
            ConfigType::Bases => "bases.config",
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

    // Paths reaching here come from config files as well as the command line, so an
    // unsupported extension is a validation error, not a reason to abort the process.
    pub fn from_path(path: &str) -> Result<FileFormat, AnyError> {
        match Path::new(path).extension().and_then(OsStr::to_str) {
            Some("yaml") => Ok(FileFormat::Yaml),
            Some(other) => Err(format!(
                "Config file format is not supported: '{}' ({})",
                other, path
            )
            .into()),
            None => Err(format!(
                "Only the yaml config file format is supported - '{}' has no '.yaml' extension",
                path
            )
            .into()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum InstallSpec {
    Script(String),
    Steps(IndexMap<String, Option<String>>),
}

impl InstallSpec {
    pub fn normalize(&self) -> IndexMap<String, Option<String>> {
        match self {
            InstallSpec::Script(script) => {
                let mut steps = IndexMap::new();
                steps.insert("00-inline".to_string(), Some(script.clone()));
                steps
            }
            InstallSpec::Steps(steps) => steps.clone(),
        }
    }

    pub fn merged(base: &Option<InstallSpec>, over: &Option<InstallSpec>) -> Option<InstallSpec> {
        match (base, over) {
            (None, None) => None,
            (Some(b), None) => Some(b.clone()),
            (None, Some(o)) => Some(o.clone()),
            (Some(b), Some(o)) => {
                let mut steps = b.normalize();
                steps.extend(o.normalize());
                Some(InstallSpec::Steps(steps))
            }
        }
    }

    pub fn resolved(&self) -> Vec<(String, String)> {
        let mut steps = self
            .normalize()
            .into_iter()
            .filter_map(|(name, script)| script.map(|s| (name, s)))
            .collect::<Vec<_>>();
        steps.sort_by(|(a, _), (b, _)| a.cmp(b));
        steps
    }
}

#[serde_with::skip_serializing_none]
#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RoozSidecar {
    pub image: Option<String>,
    pub env: Option<IndexMap<String, String>>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub mounts: Option<IndexMap<String, MountSource>>,
    pub ports: Option<Vec<String>>,
    pub privileged: Option<bool>,
    pub init: Option<bool>,
    pub install: Option<InstallSpec>,
    pub work_dir: Option<String>,
    pub user: Option<String>,
    pub uid: Option<i32>,
    pub egress: Option<bool>,
    pub shell: Option<Vec<String>>,
    pub peers: Option<Vec<String>>,
}

impl RoozSidecar {
    pub fn expand_vars(
        &self,
        reg: &Handlebars,
        vars: &IndexMap<String, String>,
    ) -> Result<Self, AnyError> {
        Ok(Self {
            image: render_opt(reg, &self.image, vars)?,
            env: render_map(reg, &self.env, vars)?,
            command: render_vec(reg, &self.command, vars)?,
            args: render_vec(reg, &self.args, vars)?,
            shell: render_vec(reg, &self.shell, vars)?,
            ports: render_vec(reg, &self.ports, vars)?,
            peers: render_vec(reg, &self.peers, vars)?,
            install: render_install(reg, &self.install, vars)?,
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
    pub bases: Option<Vec<String>>,
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
    pub install: Option<InstallSpec>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub env: Option<IndexMap<String, String>>,
    pub sidecars: Option<IndexMap<String, RoozSidecar>>,
    pub data: Option<IndexMap<String, DataValue>>,
    pub mounts: Option<IndexMap<String, MountSource>>,
}

// Rooz can defend three things for an operator: their host, their other workspaces, and
// their secrets. A field the repository being opened may author is one that reaches none of
// them - it only shapes a container that already runs that repository's code, so letting the
// repository pick the image, the shell or the install steps grants it nothing it does not
// already have. Everything else is the operator's call and is listed here.
//
// Exhaustive by construction: adding a field to RoozCfg stops this compiling until the new
// field is classified. Without that, every new feature silently becomes repo-controllable.
//
// ('privileged' is absent on purpose: rooz refuses it on any config file's say-so, including
// the operator's own, so it is gated in check_privileged rather than by provenance.)
pub fn operator_only_fields(cfg: &RoozCfg) -> Vec<&'static str> {
    let RoozCfg {
        // operator-only - reach beyond this workspace's container
        secrets, // the operator's plaintext
        caches,  // volumes shared with every other workspace of the same operator
        // repo-authorable - contained to the workspace the repository already controls
        bases,
        vars,
        git_ssh_url,
        extra_repos,
        image,
        shell,
        user,
        ports, // published on the operator's loopback only
        privileged,
        init,
        install,
        command,
        args,
        env,
        sidecars, // no ssh key mount; 'privileged' gated separately
        data,
        mounts, // rooz-managed volumes only, scoped to this workspace
    } = cfg;

    let _ = (
        bases,
        vars,
        git_ssh_url,
        extra_repos,
        image,
        shell,
        user,
        ports,
        privileged,
        init,
        install,
        command,
        args,
        env,
        sidecars,
        data,
        mounts,
    );

    let mut fields = Vec::new();
    if secrets.as_ref().is_some_and(|s| !s.is_empty()) {
        fields.push("secrets");
    }
    if caches.as_ref().is_some_and(|c| !c.is_empty()) {
        fields.push("caches");
    }
    fields
}

impl Default for RoozCfg {
    fn default() -> Self {
        Self {
            bases: None,
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

    pub fn none() -> Self {
        Self {
            bases: None,
            vars: None,
            secrets: None,
            git_ssh_url: None,
            extra_repos: None,
            image: None,
            caches: None,
            shell: None,
            user: None,
            ports: None,
            privileged: None,
            init: None,
            install: None,
            command: None,
            args: None,
            env: None,
            sidecars: None,
            data: None,
            mounts: None,
        }
    }

    pub fn from_cli(&mut self, cli: &WorkParams, shell: Option<String>) -> () {
        *self = RoozCfg {
            shell: shell.map(|v| vec![v]).or(self.shell.clone()),
            image: cli.image.clone().or(self.image.clone()),
            user: cli.user.clone().or(self.user.clone()),
            git_ssh_url: cli.git_ssh_url.clone().or(self.git_ssh_url.clone()),
            privileged: cli.privileged.or(self.privileged),
            caches: extend_if_any(self.caches.clone(), cli.caches.clone()),
            ..self.clone()
        }
    }

    pub fn from_config(&mut self, config: &RoozCfg) -> () {
        *self = RoozCfg {
            bases: None,
            vars: extend_if_any(self.vars.clone(), config.vars.clone()),
            secrets: extend_if_any(self.secrets.clone(), config.secrets.clone()),
            git_ssh_url: config.git_ssh_url.clone().or(self.git_ssh_url.clone()),
            extra_repos: extend_if_any(self.extra_repos.clone(), config.extra_repos.clone()),
            image: config.image.clone().or(self.image.clone()),
            caches: extend_if_any(self.caches.clone(), config.caches.clone()),
            shell: config.shell.clone().or(self.shell.clone()),
            user: config.user.clone().or(self.user.clone()),
            ports: extend_if_any(self.ports.clone(), config.ports.clone()),
            privileged: config.privileged.clone().or(self.privileged.clone()),
            init: config.init.clone().or(self.init.clone()),
            command: config.command.clone().or(self.command.clone()),
            args: config.args.clone().or(self.args.clone()),
            env: extend_if_any(self.env.clone(), config.env.clone()),
            sidecars: merge_sidecars(self.sidecars.clone(), config.sidecars.clone()),
            data: extend_if_any(self.data.clone(), config.data.clone()),
            mounts: extend_if_any(self.mounts.clone(), config.mounts.clone()),
            install: InstallSpec::merged(&self.install, &config.install),
        }
    }

    pub fn validate_base_path(path: &str) -> Result<(), AnyError> {
        validate_relative_config_path(path, "base path")
    }

    pub fn validate_base_list(paths: &[String]) -> Result<(), AnyError> {
        const MAX_BASE_PATHS_PER_LEVEL: usize = 7;
        if paths.len() > MAX_BASE_PATHS_PER_LEVEL {
            return Err(format!(
                "at most {} base paths allowed per level, got {}",
                MAX_BASE_PATHS_PER_LEVEL,
                paths.len()
            )
            .into());
        }
        for path in paths {
            Self::validate_base_path(path)?;
        }
        Ok(())
    }

    pub fn from_cli_env(self, cli: WorkParams) -> Self {
        RoozCfg {
            shell: cli.env.shell.map(|v| vec![v]).or(self.shell.clone()),
            image: cli.env.image.or(self.image.clone()),
            user: cli.env.user.or(self.user.clone()),
            caches: extend_if_any(self.caches.clone(), cli.env.caches),
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
    ) -> Result<&'a HashMap<String, Option<String>>, AnyError> {
        for mapping in ports.iter() {
            let (source, target) = RoozCfg::parse_port(mapping)?;
            map.insert(source.to_string(), target.map(|p| p.to_string()));
        }
        Ok(map)
    }

    // 'ports' is a list of free-form strings the repository being opened can author, so a
    // malformed mapping is a configuration error - it must not abort a run that has already
    // created volumes and containers.
    fn parse_port(port_mapping: &String) -> Result<(u16, Option<u16>), AnyError> {
        let invalid = || -> AnyError {
            format!(
                "Invalid port mapping: '{}'. Expected '<containerPort>' or \
                 '<containerPort>:<hostPort>' with ports in 0-65535.",
                port_mapping
            )
            .into()
        };
        let port = |p: &str| p.parse::<u16>().map_err(|_| invalid());

        match port_mapping.split(":").collect::<Vec<_>>().as_slice() {
            &[a] => Ok((port(a)?, None)),
            &[a, b] => Ok((port(a)?, Some(port(b)?))),
            _ => Err(invalid()),
        }
    }

    /// `allow_secrets` gates whether decrypted secrets join the render context. Every
    /// field here can be authored by the repository being opened, so a repo-provided
    /// `env: {LEAK: "{{ MY_SECRET }}"}` would otherwise hand the operator's plaintext
    /// secret to a container the repo also controls.
    pub fn expand_vars(&mut self, allow_secrets: bool) -> Result<(), AnyError> {
        if !allow_secrets && self.secrets.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err(SECRETS_NOT_ALLOWED.into());
        }

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
        self.install = render_install(&reg, &self.install, &built_vars)?;
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
// Config paths sourced from a config file (bases) or from a `--config <url>//<path>`
// fragment end up both in a shell command inside the clone container and, for local
// configs, in a host-side read. They are restricted to plain relative paths below the
// referring config's directory: no traversal, no absolute paths, no shell metacharacters.
pub fn validate_relative_config_path(path: &str, what: &str) -> Result<(), AnyError> {
    if path.is_empty() {
        return Err(format!("{} must not be empty", what).into());
    }
    if path.contains(':') {
        return Err(format!(
            "{} must be a local relative path (no URLs): '{}'",
            what, path
        )
        .into());
    }
    if path.starts_with('/') {
        return Err(format!("{} must be relative, not absolute: '{}'", what, path).into());
    }
    if path
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')))
    {
        return Err(format!(
            "{} may only contain letters, digits, '.', '_', '-' and '/': '{}'",
            what, path
        )
        .into());
    }
    if path
        .split('/')
        .any(|segment| segment == ".." || segment.is_empty())
    {
        return Err(format!(
            "{} must not traverse outside the config's directory: '{}'",
            what, path
        )
        .into());
    }
    // the format is decided by the extension, so refuse an unsupported one here - before
    // the path is handed to a clone container and read
    FileFormat::from_path(path).map_err(|_| {
        format!(
            "{} must point to a YAML config file ('*.yaml'): '{}'",
            what, path
        )
    })?;
    Ok(())
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

fn merge_sidecars(
    target: Option<IndexMap<String, RoozSidecar>>,
    other: Option<IndexMap<String, RoozSidecar>>,
) -> Option<IndexMap<String, RoozSidecar>> {
    match (target, other) {
        (Some(mut t), Some(o)) => {
            for (k, v) in o {
                match t.get_mut(&k) {
                    Some(existing) => existing.merge_from(&v),
                    None => {
                        t.insert(k, v);
                    }
                }
            }
            Some(t)
        }
        (t, None) => t,
        (None, o) => o,
    }
}

impl RoozSidecar {
    pub fn merge_from(&mut self, other: &RoozSidecar) {
        self.image = other.image.clone().or(self.image.take());
        self.env = extend_if_any(self.env.clone(), other.env.clone());
        self.command = other.command.clone().or(self.command.clone());
        self.args = other.args.clone().or(self.args.clone());
        self.shell = other.shell.clone().or(self.shell.clone());
        self.mounts = extend_if_any(self.mounts.clone(), other.mounts.clone());
        self.ports = extend_if_any(self.ports.clone(), other.ports.clone());
        self.privileged = other.privileged.or(self.privileged);
        self.init = other.init.or(self.init);
        self.install = InstallSpec::merged(&self.install, &other.install);
        self.work_dir = other.work_dir.clone().or(self.work_dir.clone());
        self.user = other.user.clone().or(self.user.clone());
        self.uid = other.uid.or(self.uid);
        self.egress = other.egress.or(self.egress);
        self.peers = union_sorted(self.peers.take(), other.peers.clone());
    }
}

fn union_sorted(target: Option<Vec<String>>, other: Option<Vec<String>>) -> Option<Vec<String>> {
    match (target, other) {
        (Some(mut t), Some(o)) => {
            t.extend(o);
            t.sort();
            t.dedup();
            Some(t)
        }
        (t, None) => t,
        (None, o) => o,
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

fn render_install(
    reg: &Handlebars,
    val: &Option<InstallSpec>,
    vars: &IndexMap<String, String>,
) -> Result<Option<InstallSpec>, AnyError> {
    val.as_ref()
        .map(|spec| {
            Ok(match spec {
                InstallSpec::Script(script) => InstallSpec::Script(render_str(reg, script, vars)?),
                InstallSpec::Steps(steps) => InstallSpec::Steps(
                    steps
                        .iter()
                        .map(|(name, script)| {
                            Ok((
                                name.clone(),
                                script
                                    .as_ref()
                                    .map(|s| render_str(reg, s, vars))
                                    .transpose()?,
                            ))
                        })
                        .collect::<Result<IndexMap<_, _>, AnyError>>()?,
                ),
            })
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
    // What may be stored on the engine: everything except the age identity, which stays on
    // the operator's machine (see util::identity).
    pub fn engine_view(&self) -> Self {
        Self {
            age_key: None,
            gitconfig: self.gitconfig.clone(),
        }
    }

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

    fn steps(entries: &[(&str, Option<&str>)]) -> InstallSpec {
        InstallSpec::Steps(
            entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.map(String::from)))
                .collect(),
        )
    }

    #[test]
    fn repo_authored_config_cannot_expand_secrets() {
        // the reported payload: a repo .rooz.yaml referencing an operator secret
        let mut cfg = RoozCfg {
            secrets: Some(IndexMap::from_iter([(
                "MY_SECRET".to_string(),
                "e2e-secret-MARKER".to_string(),
            )])),
            env: Some(IndexMap::from_iter([(
                "LEAK".to_string(),
                "{{ MY_SECRET }}".to_string(),
            )])),
            ..RoozCfg::none()
        };
        let err = cfg.expand_vars(false).unwrap_err().to_string();
        assert!(err.contains("Refusing to expand 'secrets'"), "{}", err);
        assert!(err.contains("--config"), "no remedy given: {}", err);
    }

    #[test]
    fn operator_config_still_expands_secrets() {
        let mut cfg = RoozCfg {
            secrets: Some(IndexMap::from_iter([(
                "MY_SECRET".to_string(),
                "s3cret".to_string(),
            )])),
            env: Some(IndexMap::from_iter([(
                "LEAK".to_string(),
                "{{ MY_SECRET }}".to_string(),
            )])),
            ..RoozCfg::none()
        };
        cfg.expand_vars(true).unwrap();
        assert_eq!(cfg.env.unwrap()["LEAK"], "s3cret");
    }

    #[test]
    fn vars_still_expand_for_repo_authored_config() {
        // vars are not sensitive, so repo templating keeps working
        let mut cfg = RoozCfg {
            vars: Some(IndexMap::from_iter([(
                "tag".to_string(),
                "1.2.3".to_string(),
            )])),
            image: Some("alpine:{{ tag }}".to_string()),
            ..RoozCfg::none()
        };
        cfg.expand_vars(false).unwrap();
        assert_eq!(cfg.image.unwrap(), "alpine:1.2.3");
    }

    #[test]
    fn an_empty_secrets_map_is_not_a_secret() {
        let mut cfg = RoozCfg {
            secrets: Some(IndexMap::new()),
            image: Some("alpine".to_string()),
            ..RoozCfg::none()
        };
        assert!(cfg.expand_vars(false).is_ok());
    }

    #[test]
    fn escaped_handlebars_cannot_smuggle_a_secret_through() {
        // handlebars-rust honours \{{ }} as a literal; with secrets absent from the
        // context there is no second pass that could substitute it
        let mut cfg = RoozCfg {
            vars: Some(IndexMap::from_iter([("v".to_string(), "x".to_string())])),
            env: Some(IndexMap::from_iter([(
                "LEAK".to_string(),
                "\\{{ MY_SECRET }}".to_string(),
            )])),
            ..RoozCfg::none()
        };
        cfg.expand_vars(false).unwrap();
        let rendered = cfg.env.unwrap()["LEAK"].clone();
        assert!(
            !rendered.contains("e2e-secret"),
            "unexpected substitution: {}",
            rendered
        );
    }

    #[test]
    fn hostile_config_path_fragment_is_rejected() {
        // the reported payload: `--config 'git@host:repo//cfg.yaml; touch /work/PWNED.yaml'`
        // reached `sh -c "ls {path} ... cat `ls {path} | head -1`"` in the clone container
        for spec in [
            "git@127.0.0.1:poc/repo//cfg.yaml; touch /work/PWNED6.yaml",
            "git@127.0.0.1:poc/repo//cfg.yaml; cat /tmp/.ssh/id_ed25519 > /work/key-leak.yaml",
            "git@127.0.0.1:poc/repo//x'; touch /work/PWNED.yaml; #",
            "git@127.0.0.1:poc/repo//`id`.yaml",
            "git@127.0.0.1:poc/repo//$(id).yaml",
            "git@127.0.0.1:poc/repo//../../../etc/passwd",
        ] {
            let err = ConfigPath::from_str(spec).unwrap_err().to_string();
            assert!(err.contains("config path"), "accepted {:?}: {}", spec, err);
        }
    }

    #[test]
    fn benign_config_path_fragment_is_accepted() {
        match ConfigPath::from_str("git@github.com:my/configs//path/in/repo/config.rooz.yaml") {
            Ok(ConfigPath::Git { url, file_path }) => {
                assert_eq!(url, "git@github.com:my/configs");
                assert_eq!(file_path, "path/in/repo/config.rooz.yaml");
            }
            other => panic!("expected a git config path, got {:?}", other),
        }
    }

    #[test]
    fn local_config_paths_stay_unrestricted() {
        // these are operator-typed host paths, not shell input - absolute and .. are fine
        for spec in ["../configs/my.yaml", "/home/me/my.yaml", "./my.yaml"] {
            assert!(
                matches!(ConfigPath::from_str(spec), Ok(ConfigPath::File { .. })),
                "rejected local path: {}",
                spec
            );
        }
    }

    #[test]
    fn base_path_accepts_plain_relative_paths() {
        for p in [
            "base.yaml",
            "layers/base.yaml",
            "a-b_c.1/base.yaml",
            "./base.yaml",
        ] {
            assert!(RoozCfg::validate_base_path(p).is_ok(), "rejected: {}", p);
        }
    }

    #[test]
    fn base_path_rejects_shell_metacharacters() {
        for p in [
            "x'; cp /tmp/.ssh/id_ed25519 /workspace/pwned; #",
            "x`id`.yaml",
            "x$(id).yaml",
            "a;b.yaml",
            "a|b.yaml",
            "a&b.yaml",
            "a b.yaml",
            "a\\b.yaml",
            "a\nb.yaml",
        ] {
            assert!(RoozCfg::validate_base_path(p).is_err(), "accepted: {:?}", p);
        }
    }

    #[test]
    fn base_path_rejects_traversal_and_absolute() {
        for p in [
            "",
            "../outside/host-secret.yaml",
            "../../../../etc/attacker-chosen.yaml",
            "layers/../../escape.yaml",
            "/etc/passwd",
            "https://evil/base.yaml",
            "a//b.yaml",
        ] {
            assert!(RoozCfg::validate_base_path(p).is_err(), "accepted: {:?}", p);
        }
    }

    #[test]
    fn a_non_yaml_base_is_refused_not_panicked_on() {
        // the format is picked from the extension, so an unsupported one has to be refused
        // before the file is read
        let err = RoozCfg::validate_base_path("sub/base.toml")
            .unwrap_err()
            .to_string();
        assert!(err.contains("*.yaml"), "{}", err);
        assert!(RoozCfg::validate_base_path("sub/base.yaml").is_ok());
        assert!(FileFormat::from_path("base.toml").is_err());
        assert!(FileFormat::from_path("base").is_err());
        assert!(FileFormat::from_path("base.yaml").is_ok());
    }

    #[test]
    fn malformed_port_mappings_are_refused() {
        let mut map = HashMap::new();
        for hostile in ["99999", "1:2:3", "", "a", "-1", "8080:", "8080:80/udp"] {
            let err = RoozCfg::parse_ports(&mut map, vec![hostile.to_string()])
                .unwrap_err()
                .to_string();
            assert!(err.contains(hostile), "mapping not named: {}", err);
        }
    }

    #[test]
    fn valid_port_mappings_still_parse() {
        let mut map = HashMap::new();
        RoozCfg::parse_ports(&mut map, vec!["8080".into(), "80:8081".into()]).unwrap();
        assert_eq!(map.get("8080"), Some(&None));
        assert_eq!(map.get("80"), Some(&Some("8081".to_string())));
    }

    #[test]
    fn base_list_validates_every_entry() {
        let hostile = vec!["base.yaml".to_string(), "../secret.yaml".to_string()];
        assert!(RoozCfg::validate_base_list(&hostile).is_err());
        assert!(RoozCfg::validate_base_list(&["base.yaml".to_string()]).is_ok());
    }

    #[test]
    fn install_normalize_script_to_inline_step() {
        let normalized = InstallSpec::Script("echo hi".into()).normalize();
        assert_eq!(normalized.len(), 1);
        assert_eq!(
            normalized.get("00-inline"),
            Some(&Some("echo hi".to_string()))
        );
    }

    #[test]
    fn install_merge_layer_adds_steps() {
        let base = Some(steps(&[("10-base", Some("apt-get update"))]));
        let over = Some(steps(&[("20-extra", Some("apt-get install -y jq"))]));
        let merged = InstallSpec::merged(&base, &over).unwrap();
        assert_eq!(
            merged.resolved(),
            vec![
                ("10-base".to_string(), "apt-get update".to_string()),
                ("20-extra".to_string(), "apt-get install -y jq".to_string())
            ]
        );
    }

    #[test]
    fn install_merge_layer_overrides_same_key() {
        let base = Some(steps(&[("10-base", Some("echo old"))]));
        let over = Some(steps(&[("10-base", Some("echo new"))]));
        let merged = InstallSpec::merged(&base, &over).unwrap();
        assert_eq!(
            merged.resolved(),
            vec![("10-base".to_string(), "echo new".to_string())]
        );
    }

    #[test]
    fn install_merge_tombstone_deletes_step() {
        let base = Some(steps(&[
            ("10-base", Some("echo keep")),
            ("20-gone", Some("echo drop")),
        ]));
        let over = Some(steps(&[("20-gone", None)]));
        let merged = InstallSpec::merged(&base, &over).unwrap();
        assert_eq!(
            merged.resolved(),
            vec![("10-base".to_string(), "echo keep".to_string())]
        );
    }

    #[test]
    fn install_merge_tombstone_cascades_across_layers() {
        let l1 = Some(steps(&[("10-a", Some("echo a")), ("20-b", Some("echo b"))]));
        let l2 = Some(steps(&[("20-b", None)]));
        let l3 = Some(steps(&[("30-c", Some("echo c"))]));
        let merged12 = InstallSpec::merged(&l1, &l2);
        assert_eq!(
            merged12.as_ref().unwrap().normalize().get("20-b"),
            Some(&None)
        );
        let merged = InstallSpec::merged(&merged12, &l3).unwrap();
        assert_eq!(
            merged.resolved(),
            vec![
                ("10-a".to_string(), "echo a".to_string()),
                ("30-c".to_string(), "echo c".to_string())
            ]
        );
    }

    #[test]
    fn install_merge_string_base_map_over() {
        let base = Some(InstallSpec::Script("echo base".into()));
        let over = Some(steps(&[("10-extra", Some("echo extra"))]));
        let merged = InstallSpec::merged(&base, &over).unwrap();
        assert_eq!(
            merged.resolved(),
            vec![
                ("00-inline".to_string(), "echo base".to_string()),
                ("10-extra".to_string(), "echo extra".to_string())
            ]
        );
    }

    #[test]
    fn install_merge_map_base_string_over() {
        let base = Some(steps(&[("10-extra", Some("echo extra"))]));
        let over = Some(InstallSpec::Script("echo over".into()));
        let merged = InstallSpec::merged(&base, &over).unwrap();
        assert_eq!(
            merged.resolved(),
            vec![
                ("00-inline".to_string(), "echo over".to_string()),
                ("10-extra".to_string(), "echo extra".to_string())
            ]
        );
    }

    #[test]
    fn install_merge_none_sides() {
        let some = Some(InstallSpec::Script("echo hi".into()));
        assert_eq!(InstallSpec::merged(&None, &None), None);
        assert_eq!(InstallSpec::merged(&some, &None), some);
        assert_eq!(InstallSpec::merged(&None, &some), some);
    }

    #[test]
    fn install_resolved_sorts_lexicographically_and_drops_tombstones() {
        let spec = steps(&[
            ("20-b", Some("echo b")),
            ("10-a", Some("echo a")),
            ("15-gone", None),
        ]);
        assert_eq!(
            spec.resolved(),
            vec![
                ("10-a".to_string(), "echo a".to_string()),
                ("20-b".to_string(), "echo b".to_string())
            ]
        );
    }

    #[test]
    fn sidecar_without_image_parses() {
        let yaml = "sidecars:\n  svc:\n    env:\n      A: b\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.sidecars.unwrap()["svc"].image, None);
    }

    #[test]
    fn sidecar_merge_overlay_without_image_keeps_base_image() {
        let base_yaml = "sidecars:\n  svc:\n    image: alpine\n";
        let overlay_yaml = "sidecars:\n  svc:\n    env:\n      A: b\n";
        let mut base: RoozCfg = serde_yaml::from_str(base_yaml).unwrap();
        let overlay: RoozCfg = serde_yaml::from_str(overlay_yaml).unwrap();
        base.from_config(&overlay);
        let svc = &base.sidecars.unwrap()["svc"];
        assert_eq!(svc.image, Some("alpine".to_string()));
        assert_eq!(svc.env.as_ref().unwrap()["A"], "b");
    }

    #[test]
    fn sidecar_merge_overlay_image_wins() {
        let base_yaml = "sidecars:\n  svc:\n    image: alpine\n";
        let overlay_yaml = "sidecars:\n  svc:\n    image: debian\n";
        let mut base: RoozCfg = serde_yaml::from_str(base_yaml).unwrap();
        let overlay: RoozCfg = serde_yaml::from_str(overlay_yaml).unwrap();
        base.from_config(&overlay);
        assert_eq!(
            base.sidecars.unwrap()["svc"].image,
            Some("debian".to_string())
        );
    }

    #[test]
    fn sidecar_none_image_not_serialized() {
        let cfg: RoozCfg = serde_yaml::from_str("sidecars:\n  svc:\n    uid: 1000\n").unwrap();
        let yaml = cfg.to_string(FileFormat::Yaml).unwrap();
        assert!(!yaml.contains("image"), "unexpected image key in: {}", yaml);
        let reparsed = RoozCfg::from_string(&yaml, FileFormat::Yaml).unwrap();
        assert_eq!(reparsed.sidecars.unwrap()["svc"].image, None);
    }

    #[test]
    fn sidecar_peers_parse_and_render() {
        let yaml = "vars:\n  mirror: images\nsidecars:\n  dkr:\n    image: a\n    peers: [\"{{ mirror }}\"]\n  images:\n    image: b\n";
        let mut cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        cfg.expand_vars(true).unwrap();
        assert_eq!(
            cfg.sidecars.unwrap()["dkr"].peers,
            Some(vec!["images".to_string()])
        );
    }

    #[test]
    fn sidecar_peers_merge_as_union() {
        let base_yaml = "sidecars:\n  dkr:\n    image: a\n    peers: [images]\n";
        let overlay_yaml = "sidecars:\n  dkr:\n    peers: [cache]\n";
        let mut base: RoozCfg = serde_yaml::from_str(base_yaml).unwrap();
        let overlay: RoozCfg = serde_yaml::from_str(overlay_yaml).unwrap();
        base.from_config(&overlay);
        assert_eq!(
            base.sidecars.unwrap()["dkr"].peers,
            Some(vec!["cache".to_string(), "images".to_string()])
        );
    }

    #[test]
    fn sidecar_peers_deduped_across_layers() {
        let base_yaml = "sidecars:\n  dkr:\n    image: a\n    peers: [images, cache]\n";
        let overlay_yaml = "sidecars:\n  dkr:\n    peers: [images]\n";
        let mut base: RoozCfg = serde_yaml::from_str(base_yaml).unwrap();
        let overlay: RoozCfg = serde_yaml::from_str(overlay_yaml).unwrap();
        base.from_config(&overlay);
        assert_eq!(
            base.sidecars.unwrap()["dkr"].peers,
            Some(vec!["cache".to_string(), "images".to_string()])
        );
    }

    #[test]
    fn sidecar_peers_preserved_when_overlay_has_none() {
        let base_yaml = "sidecars:\n  dkr:\n    image: a\n    peers: [images]\n";
        let overlay_yaml = "sidecars:\n  dkr:\n    env:\n      A: b\n";
        let mut base: RoozCfg = serde_yaml::from_str(base_yaml).unwrap();
        let overlay: RoozCfg = serde_yaml::from_str(overlay_yaml).unwrap();
        base.from_config(&overlay);
        assert_eq!(
            base.sidecars.unwrap()["dkr"].peers,
            Some(vec!["images".to_string()])
        );
    }

    #[test]
    fn install_serde_bare_string() {
        let yaml = "install: |\n  echo hi\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.install, Some(InstallSpec::Script("echo hi\n".into())));
    }

    #[test]
    fn install_serde_map_with_tombstone() {
        let yaml = "install:\n  10-a: echo a\n  20-b: null\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            cfg.install,
            Some(steps(&[("10-a", Some("echo a")), ("20-b", None)]))
        );
    }

    #[test]
    fn install_serde_roundtrip() {
        for spec in [
            InstallSpec::Script("echo hi".into()),
            steps(&[("10-a", Some("echo a")), ("20-b", None)]),
        ] {
            let cfg = RoozCfg {
                install: Some(spec.clone()),
                ..RoozCfg::none()
            };
            let yaml = cfg.to_string(FileFormat::Yaml).unwrap();
            let parsed = RoozCfg::from_string(&yaml, FileFormat::Yaml).unwrap();
            assert_eq!(parsed.install, Some(spec));
        }
    }

    #[test]
    fn install_templating_renders_step_values() {
        let mut cfg = RoozCfg {
            vars: Some(IndexMap::from_iter([("pkg".to_string(), "jq".to_string())])),
            install: Some(steps(&[
                ("10-a", Some("apk add {{pkg}}")),
                ("20-gone", None),
            ])),
            ..RoozCfg::none()
        };
        cfg.expand_vars(true).unwrap();
        assert_eq!(
            cfg.install,
            Some(steps(&[("10-a", Some("apk add jq")), ("20-gone", None)]))
        );
    }

    #[test]
    fn install_templating_renders_script_variant() {
        let mut cfg = RoozCfg {
            vars: Some(IndexMap::from_iter([("pkg".to_string(), "jq".to_string())])),
            install: Some(InstallSpec::Script("apk add {{pkg}}".into())),
            ..RoozCfg::none()
        };
        cfg.expand_vars(true).unwrap();
        assert_eq!(cfg.install, Some(InstallSpec::Script("apk add jq".into())));
    }

    #[test]
    fn deny_unknown_fields_on_parent_structs() {
        assert!(serde_yaml::from_str::<RoozCfg>("bogus_field: x\n").is_err());
        assert!(serde_yaml::from_str::<RoozSidecar>("image: alpine\nbogus_field: x\n").is_err());
    }

    #[test]
    fn install_example_parses() {
        let yaml = include_str!("../../examples/install/install.yaml");
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        let resolved = cfg.install.unwrap().resolved();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].0, "00-tools");
        let sidecar_install = cfg.sidecars.unwrap()["test"].install.clone().unwrap();
        assert_eq!(sidecar_install.resolved()[0].0, "00-inline");
    }

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
