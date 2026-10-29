use crate::model::types::ContentGenerator::{Inline, Script};
use crate::model::types::{AnyError, ContentGenerator, DataEntryKey};
use crate::util::id;
use crate::{cli::WorkParams, constants};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use handlebars::{Handlebars, no_escape};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

// The refusals of a configuration that is not wholly the operator's: what was refused on one line,
// how to allow it and where the reasoning is on the next. The docs explain the why.
#[macro_export]
macro_rules! not_yours {
    () => {
        "a configuration not wholly yours (a remote '--config' source or a 'bases' layer not in ROOZ_TRUSTED_CONFIGS)"
    };
}

#[macro_export]
macro_rules! trust_hint {
    () => {
        "Trust it with ROOZ_TRUSTED_CONFIGS=<repository url or directory>, or use your own local '--config' file"
    };
}

pub const ROOT_STEPS_NOT_ALLOWED: &str = concat!(
    "Refusing install steps with 'root: true' from ",
    not_yours!(),
    ":"
);

pub const ROOT_STEPS_HINT: &str = concat!(
    trust_hint!(),
    ", or drop 'root: true' - see https://rooz.dev/configuration.html#remote-config"
);

pub const SECRETS_NOT_ALLOWED: &str = concat!(
    "Refusing to expand 'secrets' from ",
    not_yours!(),
    ".\n",
    trust_hint!(),
    " - see https://rooz.dev/secrets.html#trusted-config-sources"
);

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

impl FromStr for ConfigPath {
    type Err = AnyError;

    fn from_str(value: &str) -> Result<Self, AnyError> {
        if !value.contains(':') {
            return Ok(Self::File {
                path: value.to_string(),
            });
        }
        // remote configs are cloned with rooz's ssh key, so only ssh transports apply
        let not_ssh = || -> AnyError {
            format!(
                "Remote --config must use SSH: 'git@host:repo//path.yaml' or \
                 'ssh://git@host[:port]/repo//path.yaml', got '{}'",
                value
            )
            .into()
        };
        let (scheme, rest) = match value.split_once("://") {
            Some((scheme, rest)) => {
                if !matches!(
                    scheme.to_ascii_lowercase().as_str(),
                    "ssh" | "git+ssh" | "ssh+git"
                ) {
                    return Err(not_ssh());
                }
                (&value[..scheme.len() + 3], rest)
            }
            // 'name::address' is a remote helper, not the scp-like ssh form
            None if value.contains("::") => return Err(not_ssh()),
            None => ("", value),
        };
        match *rest.split("//").collect::<Vec<_>>().as_slice() {
            [repo, file_path] => {
                // reject the fragment here rather than at the sink: it reaches a shell
                // in the clone container, and FileFormat::from_path would otherwise
                // panic on it first
                validate_relative_config_path(file_path, "config path")?;
                Ok(Self::Git {
                    url: format!("{}{}", scheme, repo),
                    file_path: file_path.to_string(),
                })
            }
            _ => Err(format!("Invalid remote config spec URL {}", value).into()),
        }
    }
}

impl fmt::Display for ConfigPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigPath::File { path } => write!(f, "{}", path),
            ConfigPath::Git { url, file_path } => write!(f, "{}//{}", url, file_path),
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

impl fmt::Display for FileFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileFormat::Yaml => write!(f, "yaml"),
        }
    }
}

impl FileFormat {
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum InstallSpec {
    Script(String),
    Steps(IndexMap<String, Option<InstallStep>>),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
#[serde(untagged)]
pub enum InstallStep {
    Script(String),
    Detailed(InstallStepSpec),
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallStepSpec {
    pub run: String,
    pub skip_if: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub root: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStep {
    pub name: String,
    pub run: String,
    pub skip_if: Option<String>,
    pub root: bool,
}

impl From<&str> for InstallStep {
    fn from(script: &str) -> Self {
        InstallStep::Script(script.to_string())
    }
}

impl InstallStep {
    pub fn texts_mut(&mut self) -> Vec<&mut String> {
        match self {
            InstallStep::Script(run) => vec![run],
            InstallStep::Detailed(d) => std::iter::once(&mut d.run)
                .chain(d.skip_if.as_mut())
                .collect(),
        }
    }

    fn resolve(&self, name: &str) -> ResolvedStep {
        let (run, skip_if, root) = match self {
            InstallStep::Script(run) => (run.clone(), None, false),
            InstallStep::Detailed(d) => (d.run.clone(), d.skip_if.clone(), d.root),
        };
        ResolvedStep {
            name: name.to_string(),
            run,
            skip_if,
            root,
        }
    }
}

impl InstallSpec {
    pub fn normalize(&self) -> IndexMap<String, Option<InstallStep>> {
        match self {
            InstallSpec::Script(script) => {
                let mut steps = IndexMap::new();
                steps.insert(
                    "00-inline".to_string(),
                    Some(InstallStep::Script(script.clone())),
                );
                steps
            }
            InstallSpec::Steps(steps) => steps.clone(),
        }
    }

    pub fn root_steps(&self) -> Vec<String> {
        self.resolved()
            .into_iter()
            .filter(|step| step.root)
            .map(|step| step.name)
            .collect()
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

    pub fn resolved(&self) -> Vec<ResolvedStep> {
        let mut steps = self
            .normalize()
            .into_iter()
            .filter_map(|(name, step)| step.map(|s| s.resolve(&name)))
            .collect::<Vec<_>>();
        steps.sort_by(|a, b| a.name.cmp(&b.name));
        steps
    }
}

#[serde_with::skip_serializing_none]
#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoozSidecar {
    pub image: Option<String>,
    pub env: Option<IndexMap<String, String>>,
    pub command: Option<Vec<String>>,
    pub args: Option<Vec<String>>,
    pub mounts: Option<IndexMap<String, MountSource>>,
    pub ports: Option<Vec<String>>,
    pub privileged: Option<bool>,
    pub vpn: Option<bool>,
    pub init: Option<bool>,
    pub install: Option<InstallSpec>,
    pub work_dir: Option<String>,
    pub uid: Option<i32>,
    pub egress: Option<crate::config::egress::SidecarEgress>,
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
            vpn: self.vpn,
            init: self.init,
            uid: self.uid,
            egress: self
                .egress
                .as_ref()
                .map(|e| e.expand_vars(reg, vars))
                .transpose()?,
        })
    }
}

#[serde_with::skip_serializing_none]
#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
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
// their secrets. A field an untrusted config may set is one that reaches none of
// them - it only shapes the workspace's own container, so letting it pick the image, the shell
// or the install steps grants it nothing it does not already have. Everything else is the operator's call and is listed here.
//
// Exhaustive by construction: adding a field to RoozCfg stops this compiling until the new
// field is classified. Without that, every new feature silently becomes settable by an untrusted config.
//
// ('privileged' is absent on purpose: rooz refuses it on any config file's say-so, including
// the operator's own, so it is gated in check_privileged rather than by provenance.)
pub fn operator_only_fields(cfg: &RoozCfg) -> Vec<&'static str> {
    let RoozCfg {
        // operator-only - reach beyond this workspace's container
        secrets,     // the operator's plaintext
        caches,      // volumes shared with every other workspace of the same operator
        extra_repos, // cloned by a container that mounts the operator's ssh key
        // settable by any config - contained to the workspace
        bases,
        vars,
        git_ssh_url,
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
    if extra_repos.as_ref().is_some_and(|r| !r.is_empty()) {
        fields.push("extra_repos");
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
    pub fn root_install_steps(&self) -> Vec<String> {
        let own = self.install.iter().flat_map(|i| i.root_steps());
        let sidecars = self.sidecars.iter().flatten().flat_map(|(sidecar, sc)| {
            sc.install
                .iter()
                .flat_map(|i| i.root_steps())
                .map(move |name| format!("{}/{}", sidecar, name))
        });
        own.chain(sidecars).collect()
    }

    pub fn from_string(config: &str, file_format: FileFormat) -> Result<Self, AnyError> {
        Ok(match file_format {
            FileFormat::Yaml => serde_yaml::from_str(config)?,
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

    pub fn from_cli(&mut self, cli: &WorkParams, shell: Option<String>) {
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

    pub fn from_config(&mut self, config: &RoozCfg) {
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
            privileged: config.privileged.or(self.privileged),
            init: config.init.or(self.init),
            command: config.command.clone().or(self.command.clone()),
            args: config.args.clone().or(self.args.clone()),
            env: extend_if_any(self.env.clone(), config.env.clone()),
            sidecars: merge_sidecars(self.sidecars.clone(), config.sidecars.clone()),
            data: extend_if_any(self.data.clone(), config.data.clone()),
            mounts: extend_if_any(self.mounts.clone(), config.mounts.clone()),
            install: InstallSpec::merged(&self.install, &config.install),
        }
    }

    // serde_yaml reads an unquoted `1` or `true` into a string field, and configs rely on it
    // (`vars: {x: 1}`), so the schema accepts those scalars wherever it says string.
    pub fn json_schema() -> Result<String, AnyError> {
        fn widen(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Object(map) => {
                    if !map.contains_key("enum")
                        && !map.contains_key("const")
                        && let Some(t) = map.get_mut("type")
                    {
                        let mut types = match t.take() {
                            serde_json::Value::Array(a) => a,
                            other => vec![other],
                        };
                        if types.iter().any(|x| x == "string") {
                            for extra in ["number", "boolean"] {
                                if !types.iter().any(|x| x == extra) {
                                    types.push(extra.into());
                                }
                            }
                        }
                        *t = if types.len() == 1 {
                            types.remove(0)
                        } else {
                            types.into()
                        };
                    }
                    map.values_mut().for_each(widen);
                }
                serde_json::Value::Array(items) => items.iter_mut().for_each(widen),
                _ => (),
            }
        }
        let mut schema = serde_json::to_value(schemars::schema_for!(RoozCfg))?;
        widen(&mut schema);
        Ok(serde_json::to_string_pretty(&schema)?)
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

    pub fn expand_dir_mounts(&mut self) -> Result<(), AnyError> {
        expand_dir_mounts(&mut self.mounts)?;
        for sidecar in self.sidecars.iter_mut().flat_map(|s| s.values_mut()) {
            expand_dir_mounts(&mut sidecar.mounts)?;
        }
        Ok(())
    }

    pub fn dir_sources_mut(&mut self) -> Vec<&mut DirSource> {
        fn dirs(mounts: &mut Option<IndexMap<String, MountSource>>) -> Vec<&mut DirSource> {
            mounts
                .iter_mut()
                .flat_map(|m| m.values_mut())
                .filter_map(|m| match m {
                    MountSource::InlineDir(d) => Some(d),
                    _ => None,
                })
                .collect()
        }
        let mut sources = dirs(&mut self.mounts);
        for sidecar in self.sidecars.iter_mut().flat_map(|s| s.values_mut()) {
            sources.extend(dirs(&mut sidecar.mounts));
        }
        sources
    }

    pub fn data_values_mut(&mut self) -> Vec<&mut DataValue> {
        fn inline(mounts: &mut Option<IndexMap<String, MountSource>>) -> Vec<&mut DataValue> {
            mounts
                .iter_mut()
                .flat_map(|m| m.values_mut())
                .filter_map(|m| match m {
                    MountSource::InlineDataValue(v) => Some(v),
                    MountSource::DataEntryReference(_) | MountSource::InlineDir(_) => None,
                })
                .collect()
        }
        let mut values: Vec<&mut DataValue> =
            self.data.iter_mut().flat_map(|d| d.values_mut()).collect();
        values.extend(inline(&mut self.mounts));
        for sidecar in self.sidecars.iter_mut().flat_map(|s| s.values_mut()) {
            values.extend(inline(&mut sidecar.mounts));
        }
        values
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
            .or(cli_cfg.clone().and_then(|c| c.git_ssh_url))
    }

    pub fn parse_ports(
        map: &mut HashMap<String, Option<String>>,
        ports: Vec<String>,
    ) -> Result<&HashMap<String, Option<String>>, AnyError> {
        for mapping in ports.iter() {
            let (source, target) = RoozCfg::parse_port(mapping)?;
            map.insert(source.to_string(), target.map(|p| p.to_string()));
        }
        Ok(map)
    }

    // 'ports' is a list of free-form strings a remote config can author, so a
    // malformed mapping is a configuration error - it must not abort a run that has already
    // created volumes and containers.
    fn parse_port(port_mapping: &String) -> Result<(u16, Option<u16>), AnyError> {
        let invalid = || -> AnyError {
            format!(
                "Invalid port mapping: '{}'. Expected '<containerPort>' or \
                 '<hostPort>:<containerPort>' with ports in 0-65535.",
                port_mapping
            )
            .into()
        };
        let port = |p: &str| p.parse::<u16>().map_err(|_| invalid());

        match *port_mapping.split(":").collect::<Vec<_>>().as_slice() {
            [a] => Ok((port(a)?, None)),
            [host, container] => Ok((port(container)?, Some(port(host)?))),
            _ => Err(invalid()),
        }
    }

    /// `allow_secrets` gates whether decrypted secrets join the render context. Every
    /// field here can come from a remote config, so a `env: {LEAK: "{{ MY_SECRET }}"}` in
    /// one would otherwise hand the operator's plaintext secret to a container the config
    /// also controls.
    pub fn expand_vars(&mut self, allow_secrets: bool) -> Result<(), AnyError> {
        if !allow_secrets && self.secrets.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err(SECRETS_NOT_ALLOWED.into());
        }

        let secrets = self.secrets.clone().unwrap_or_default();
        let vars = self.vars.clone().unwrap_or_default();
        if let Some(duplicate_key) = vars.keys().find(|k| secrets.contains_key(*k)) {
            return Err(format!(
                "The key '{}' is defined in both 'vars' and 'secrets'; it can only be in one.",
                duplicate_key
            )
            .into());
        }

        let mut reg = Handlebars::new();
        reg.register_escape_fn(no_escape);
        // a variable nothing defines is a typo, not an empty string
        reg.set_strict_mode(true);
        // a secret is a value, never a template: rendering it would let a '{{' in the plaintext
        // corrupt it, escape masking, or quote it in a template error
        let mut built_vars = secrets;
        let mut rendered_vars = IndexMap::new();
        for (k, v) in vars {
            let rendered = reg.render_template(&v, &built_vars)?;
            rendered_vars.insert(k.clone(), rendered.clone());
            built_vars.insert(k, rendered);
        }

        // rendered once, in order: a variable can use the ones before it, and its value is
        // never run through the template engine a second time
        self.vars = self.vars.as_ref().map(|_| rendered_vars);
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

    pub fn deserialize_config(config: &str, file_format: FileFormat) -> Result<RoozCfg, AnyError> {
        RoozCfg::from_string(config, file_format)
            .map_err(|e| format!("Could not read config ({}): {}", file_format, e).into())
    }
}
// Config paths sourced from a config file (bases) or from a `--config <url>//<path>`
// fragment end up both in a shell command inside the clone container and, for local
// configs, in a host-side read. They are restricted to plain relative paths below the
// referring config's directory: no traversal, no absolute paths, no shell metacharacters.
pub fn validate_relative_config_path(path: &str, what: &str) -> Result<(), AnyError> {
    validate_relative_path(path, what)?;
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

pub const MAX_DATA_FILE_BYTES: usize = 64 * 1024;

pub fn validate_relative_file_path(path: &str) -> Result<(), AnyError> {
    validate_relative_path(path, "data file path")?;
    if path.split('/').any(|segment| segment == ".") {
        return Err(format!("data file path must not contain '.' segments: '{}'", path).into());
    }
    Ok(())
}

fn validate_relative_path(path: &str, what: &str) -> Result<(), AnyError> {
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
        self.vpn = other.vpn.or(self.vpn);
        self.init = other.init.or(self.init);
        self.install = InstallSpec::merged(&self.install, &other.install);
        self.work_dir = other.work_dir.clone().or(self.work_dir.clone());
        self.uid = other.uid.or(self.uid);
        self.egress = crate::config::egress::merged(self.egress.take(), other.egress.clone());
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
                        .map(|(name, step)| {
                            let step = step
                                .as_ref()
                                .map(|st| {
                                    let mut st = st.clone();
                                    for text in st.texts_mut() {
                                        *text = render_str(reg, text, vars)?;
                                    }
                                    Ok::<_, AnyError>(st)
                                })
                                .transpose()?;
                            Ok((name.clone(), step))
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
        Ok(serde_yaml::from_str(config)?)
    }

    pub fn to_string(config: &Self) -> Result<String, AnyError> {
        Ok(serde_yaml::to_string(&config)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum DataValue {
    Dir {},
    InlineContent {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        executable: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        once: Option<bool>,
    },
    GeneratedContent {
        generate: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        image: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        executable: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        once: Option<bool>,
    },
    FileContent {
        file: String,
        #[serde(skip)]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        executable: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        once: Option<bool>,
    },
}

impl DataValue {
    pub fn into_entry(self, name: String) -> DataEntry {
        match self {
            DataValue::InlineContent {
                content,
                executable,
                once,
            } => DataEntry::File {
                name,
                generator: Inline(content),
                executable: executable.unwrap_or_default(),
                once: once.unwrap_or_default(),
            },
            DataValue::GeneratedContent {
                generate,
                image,
                executable,
                once,
            } => DataEntry::File {
                name,
                generator: Script {
                    script: generate,
                    image,
                },
                executable: executable.unwrap_or_default(),
                once: once.unwrap_or_default(),
            },
            DataValue::FileContent {
                content,
                executable,
                once,
                ..
            } => DataEntry::File {
                name,
                generator: Inline(content.unwrap_or_default()),
                executable: executable.unwrap_or_default(),
                once: once.unwrap_or_default(),
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
                once,
            } => DataValue::InlineContent {
                content: render_str(reg, content, vars)?,
                executable: *executable,
                once: *once,
            },
            DataValue::GeneratedContent {
                generate,
                image,
                executable,
                once,
            } => DataValue::GeneratedContent {
                generate: render_str(reg, generate, vars)?,
                image: render_opt(reg, image, vars)?,
                executable: *executable,
                once: *once,
            },
            DataValue::FileContent {
                file,
                content,
                executable,
                once,
            } => DataValue::InlineContent {
                content: content.clone().ok_or_else(|| {
                    format!(
                        "data file '{}' was not read. 'file:' is resolved when the config is loaded from its origin, which is not possible here (e.g. 'update --tweak' on a config whose origin is gone)",
                        file
                    )
                })?,
                executable: *executable,
                once: *once,
            },
        })
    }
}

pub const MAX_DIR_FILES: usize = 256;
pub const MAX_DIR_BYTES: usize = 1024 * 1024;

// A file read from a `dir:` source: its path below the dir, its text and its executable bit
#[derive(Debug, Clone)]
pub struct DirFile {
    pub path: String,
    pub content: String,
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DirSource {
    pub dir: String,
    #[serde(skip)]
    pub files: Option<Vec<DirFile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub once: Option<bool>,
    // globs relative to the dir: only the files these match, when given
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    // globs relative to the dir: files and whole directories left out
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

// Which entries of a `dir:` source are mounted: the files `include` matches (all, without it) that
// neither `exclude` nor a directory `exclude` matches holds. What is left out is not looked at.
#[derive(Default)]
pub struct DirFilter {
    include: Option<GlobSet>,
    exclude: GlobSet,
}

impl DirFilter {
    pub fn new(source: &DirSource) -> Result<Self, AnyError> {
        fn set(globs: &[String]) -> Result<GlobSet, AnyError> {
            let mut set = GlobSetBuilder::new();
            for g in globs {
                set.add(
                    GlobBuilder::new(g)
                        .literal_separator(true)
                        .build()
                        .map_err(|e| format!("'dir: {}' glob {}", g, e))?,
                );
            }
            Ok(set.build()?)
        }
        Ok(Self {
            include: source.include.as_deref().map(set).transpose()?,
            exclude: set(source.exclude.as_deref().unwrap_or_default())?,
        })
    }

    // a directory below the dir, by its path relative to it, skipped whole
    pub fn skips_dir(&self, path: &str) -> bool {
        self.exclude.is_match(path)
    }

    // an entry other than a directory, by its path relative to the dir
    pub fn keeps(&self, path: &str) -> bool {
        let mut parent = path;
        while let Some((above, _)) = parent.rsplit_once('/') {
            if self.skips_dir(above) {
                return false;
            }
            parent = above;
        }
        !self.exclude.is_match(path) && self.include.as_ref().is_none_or(|i| i.is_match(path))
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum MountSource {
    DataEntryReference(DataEntryKey),
    InlineDir(DirSource),
    InlineDataValue(DataValue),
}

// Untagged, a mistake in a mount only says it matched no variant. A `dir:` key picks the
// variant, so its own error (an unknown field, a wrong type) comes through.
impl<'de> Deserialize<'de> for MountSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let parsed = if value.is_string() {
            DataEntryKey::deserialize(value).map(MountSource::DataEntryReference)
        } else if value.as_mapping().is_some_and(|m| m.contains_key("dir")) {
            DirSource::deserialize(value).map(MountSource::InlineDir)
        } else {
            DataValue::deserialize(value).map(MountSource::InlineDataValue)
        };
        parsed.map_err(serde::de::Error::custom)
    }
}

// A `dir:` mount becomes a `file:` mount per file below it, at the same path below the mount's.
// It runs on the merged config, so a path any layer mounts on its own keeps that mount.
fn expand_dir_mounts(mounts: &mut Option<IndexMap<String, MountSource>>) -> Result<(), AnyError> {
    let Some(mounts) = mounts else {
        return Ok(());
    };
    if !mounts
        .values()
        .any(|m| matches!(m, MountSource::InlineDir(_)))
    {
        return Ok(());
    }
    let (dirs, mut expanded): (Vec<_>, Vec<_>) = std::mem::take(mounts)
        .into_iter()
        .partition(|(_, m)| matches!(m, MountSource::InlineDir(_)));
    let explicit = expanded
        .iter()
        .map(|(target, _)| target.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut from_dirs = std::collections::HashSet::new();
    for (target, source) in dirs {
        let MountSource::InlineDir(d) = source else {
            continue;
        };
        let files = d.files.ok_or_else(|| {
            format!(
                "mount '{}': dir '{}' was not read. 'dir:' is resolved when the config is loaded from its origin, which is not possible here (e.g. 'update --tweak' on a config whose origin is gone)",
                target, d.dir
            )
        })?;
        for f in files {
            let path = format!("{}/{}", target.trim_end_matches('/'), f.path);
            if explicit.contains(&path) {
                continue;
            }
            if !from_dirs.insert(path.clone()) {
                return Err(
                    format!("mount '{}': two 'dir:' mounts provide this path", path).into(),
                );
            }
            expanded.push((
                path,
                MountSource::InlineDataValue(DataValue::FileContent {
                    file: format!("{}/{}", d.dir, f.path),
                    content: Some(f.content),
                    executable: Some(d.executable.unwrap_or(f.executable)),
                    once: d.once,
                }),
            ));
        }
    }
    *mounts = expanded.into_iter().collect();
    Ok(())
}

impl MountSource {
    // An inline file lives in the inline volume under a name derived from its path - and, in a
    // sidecar, from the sidecar's name too, so the same path in two containers stays two files.
    pub fn resolve_key(&self, container: &str, target: &str) -> String {
        match self {
            MountSource::DataEntryReference(data_key) => data_key.as_str().to_string(),
            MountSource::InlineDataValue(_) | MountSource::InlineDir(_)
                if container == crate::constants::DEFAULT_CONTAINER_NAME =>
            {
                id::sanitize(target)
            }
            MountSource::InlineDataValue(_) | MountSource::InlineDir(_) => {
                format!("_{}_{}", container, id::sanitize(target))
            }
        }
    }

    pub fn expand_vars(
        &self,
        reg: &Handlebars,
        vars: &IndexMap<String, String>,
    ) -> Result<Self, AnyError> {
        Ok(match self {
            MountSource::DataEntryReference(k) => MountSource::DataEntryReference(k.clone()),
            MountSource::InlineDir(d) => MountSource::InlineDir(d.clone()),
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
        once: bool,
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

    fn pairs(spec: &InstallSpec) -> Vec<(String, String)> {
        spec.resolved()
            .into_iter()
            .map(|s| (s.name, s.run))
            .collect()
    }

    fn steps(entries: &[(&str, Option<&str>)]) -> InstallSpec {
        InstallSpec::Steps(
            entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.map(InstallStep::from)))
                .collect(),
        )
    }

    #[test]
    fn install_step_object_form_parses() {
        let cfg: RoozCfg = serde_yaml::from_str(
            "install:\n  10-a: echo a\n  20-b:\n    run: echo b\n    skip_if: command -v b\n  30-c:\n    run: apk add jq\n    root: true\n  40-gone: null\n",
        )
        .unwrap();
        assert_eq!(
            cfg.install.unwrap().resolved(),
            vec![
                ResolvedStep {
                    name: "10-a".into(),
                    run: "echo a".into(),
                    skip_if: None,
                    root: false
                },
                ResolvedStep {
                    name: "20-b".into(),
                    run: "echo b".into(),
                    skip_if: Some("command -v b".into()),
                    root: false
                },
                ResolvedStep {
                    name: "30-c".into(),
                    run: "apk add jq".into(),
                    skip_if: None,
                    root: true
                },
            ]
        );
    }

    #[test]
    fn install_step_rejects_unknown_fields() {
        assert!(
            serde_yaml::from_str::<RoozCfg>("install:\n  10-a:\n    run: x\n    user: root\n")
                .is_err()
        );
    }

    #[test]
    fn install_step_overlay_replaces_whole_step() {
        let base: RoozCfg = serde_yaml::from_str(
            "install:\n  10-a:\n    run: echo old\n    skip_if: 'true'\n    root: true\n",
        )
        .unwrap();
        let over: RoozCfg = serde_yaml::from_str("install:\n  10-a: echo new\n").unwrap();
        let merged = InstallSpec::merged(&base.install, &over.install).unwrap();
        assert_eq!(
            merged.resolved(),
            vec![ResolvedStep {
                name: "10-a".into(),
                run: "echo new".into(),
                skip_if: None,
                root: false
            }]
        );
    }

    #[test]
    fn root_install_steps_lists_main_and_sidecar_steps() {
        let cfg: RoozCfg = serde_yaml::from_str(
            "install:\n  10-a: echo a\n  20-b:\n    run: apk add x\n    root: true\nsidecars:\n  db:\n    image: x\n    install:\n      10-pkg:\n        run: apk add y\n        root: true\n",
        )
        .unwrap();
        assert_eq!(cfg.root_install_steps(), vec!["20-b", "db/10-pkg"]);
        let plain: RoozCfg = serde_yaml::from_str("install: echo hi\n").unwrap();
        assert!(plain.root_install_steps().is_empty());
    }

    #[test]
    fn install_step_templates_run_and_skip_if() {
        let mut cfg: RoozCfg = serde_yaml::from_str(
            "vars:\n  v: \"8.0\"\ninstall:\n  10-a:\n    run: install {{ v }}\n    skip_if: check {{ v }}\n",
        )
        .unwrap();
        cfg.expand_vars(true).unwrap();
        let step = cfg.install.unwrap().resolved().remove(0);
        assert_eq!(step.run, "install 8.0");
        assert_eq!(step.skip_if.as_deref(), Some("check 8.0"));
    }

    #[test]
    fn untrusted_config_cannot_expand_secrets() {
        // the reported payload: a remote config referencing an operator secret
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
    fn a_secret_is_a_value_not_a_template() {
        for plaintext in ["hunter2{{oops", "tok{{b}}en"] {
            let mut cfg = RoozCfg {
                secrets: Some(IndexMap::from_iter([(
                    "TOKEN".to_string(),
                    plaintext.to_string(),
                )])),
                vars: Some(IndexMap::from_iter([("b".to_string(), "X".to_string())])),
                env: Some(IndexMap::from_iter([(
                    "TOKEN".to_string(),
                    "{{ TOKEN }}".to_string(),
                )])),
                ..RoozCfg::none()
            };
            cfg.expand_vars(true).unwrap();
            assert_eq!(cfg.env.unwrap()["TOKEN"], plaintext);
        }
    }

    #[test]
    fn a_key_in_both_vars_and_secrets_is_an_error_not_a_panic() {
        let mut cfg = RoozCfg {
            secrets: Some(IndexMap::from_iter([("K".to_string(), "s".to_string())])),
            vars: Some(IndexMap::from_iter([("K".to_string(), "v".to_string())])),
            ..RoozCfg::none()
        };
        let err = cfg.expand_vars(true).unwrap_err().to_string();
        assert!(err.contains("'K'"), "{}", err);
    }

    #[test]
    fn vars_still_expand_for_untrusted_config() {
        // vars are not sensitive, so templating keeps working for untrusted config
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
    fn an_undefined_variable_is_an_error_naming_it() {
        let mut cfg = RoozCfg {
            image: Some("alpine:{{ tga }}".to_string()),
            ..RoozCfg::none()
        };
        let err = cfg.expand_vars(false).unwrap_err().to_string();
        assert!(err.contains("tga"), "{}", err);
    }

    #[test]
    fn vars_expand_in_order_and_once() {
        let vars = |pairs: &[(&str, &str)]| {
            Some(IndexMap::from_iter(
                pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())),
            ))
        };
        let mut cfg = RoozCfg {
            vars: vars(&[("a", "x"), ("b", "{{a}}-y")]),
            image: Some("{{ b }}".to_string()),
            ..RoozCfg::none()
        };
        cfg.expand_vars(false).unwrap();
        assert_eq!(cfg.vars, vars(&[("a", "x"), ("b", "x-y")]));
        assert_eq!(cfg.image.unwrap(), "x-y");

        let mut forward = RoozCfg {
            vars: vars(&[("a", "{{b}}"), ("b", "x")]),
            ..RoozCfg::none()
        };
        assert!(forward.expand_vars(false).is_err());
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
    fn ssh_url_config_path_is_accepted() {
        for (spec, want) in [
            (
                "ssh://git@host:2222/my/configs//cfg.yaml",
                "ssh://git@host:2222/my/configs",
            ),
            (
                "git+ssh://git@host/my/configs//cfg.yaml",
                "git+ssh://git@host/my/configs",
            ),
        ] {
            match ConfigPath::from_str(spec) {
                Ok(ConfigPath::Git { url, file_path }) => {
                    assert_eq!(url, want);
                    assert_eq!(file_path, "cfg.yaml");
                }
                other => panic!("expected a git config path for {}, got {:?}", spec, other),
            }
        }
    }

    #[test]
    fn non_ssh_config_urls_are_refused() {
        for spec in [
            "https://github.com/my/configs//cfg.yaml",
            "http://host/my/configs//cfg.yaml",
            "git://host/my/configs//cfg.yaml",
            "file:///srv/configs//cfg.yaml",
            "ext::sh -c id//cfg.yaml",
        ] {
            let err = ConfigPath::from_str(spec).unwrap_err().to_string();
            assert!(err.contains("must use SSH"), "{}: {}", spec, err);
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
        RoozCfg::parse_ports(&mut map, vec!["8080".into(), "8081:80".into()]).unwrap();
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
            Some(&Some(InstallStep::from("echo hi")))
        );
    }

    #[test]
    fn install_merge_layer_adds_steps() {
        let base = Some(steps(&[("10-base", Some("apt-get update"))]));
        let over = Some(steps(&[("20-extra", Some("apt-get install -y jq"))]));
        let merged = InstallSpec::merged(&base, &over).unwrap();
        assert_eq!(
            pairs(&merged),
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
            pairs(&merged),
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
            pairs(&merged),
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
            pairs(&merged),
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
            pairs(&merged),
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
            pairs(&merged),
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
            pairs(&spec),
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
    fn sidecar_vpn_overlay_wins_and_absent_keeps_base() {
        let mut base: RoozCfg =
            serde_yaml::from_str("sidecars:\n  wg:\n    image: a\n    vpn: true\n").unwrap();
        base.from_config(&serde_yaml::from_str("sidecars:\n  wg:\n    uid: 0\n").unwrap());
        assert_eq!(base.sidecars.as_ref().unwrap()["wg"].vpn, Some(true));
        base.from_config(&serde_yaml::from_str("sidecars:\n  wg:\n    vpn: false\n").unwrap());
        assert_eq!(base.sidecars.unwrap()["wg"].vpn, Some(false));
    }

    #[test]
    fn file_reference_parses_and_survives_serialization_as_a_reference() {
        let yaml = "data:\n  run.sh:\n    file: files/run.sh\n    executable: true\n";
        let mut cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        if let DataValue::FileContent { content, .. } = &mut cfg.data.as_mut().unwrap()["run.sh"] {
            *content = Some("echo {{ not_a_var }}\n".into());
        } else {
            panic!("expected a file reference");
        }
        let stored = serde_yaml::to_string(&cfg).unwrap();
        assert!(stored.contains("file: files/run.sh"), "{}", stored);
        assert!(!stored.contains("echo"), "{}", stored);
    }

    #[test]
    fn unresolved_file_reference_is_refused_on_expand() {
        let mut cfg: RoozCfg = serde_yaml::from_str("data:\n  a:\n    file: a.txt\n").unwrap();
        let err = cfg.expand_vars(true).unwrap_err().to_string();
        assert!(err.contains("a.txt"), "{}", err);
    }

    #[test]
    fn file_content_is_used_verbatim_not_templated() {
        let yaml = "vars:\n  x: expanded\nmounts:\n  /etc/a:\n    file: a.conf\n";
        let mut cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        for v in cfg.data_values_mut() {
            if let DataValue::FileContent { content, .. } = v {
                *content = Some("{{ x }} and {{ missing".into());
            }
        }
        cfg.expand_vars(true).unwrap();
        match &cfg.mounts.unwrap()["/etc/a"] {
            MountSource::InlineDataValue(DataValue::InlineContent { content, .. }) => {
                assert_eq!(content, "{{ x }} and {{ missing")
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn data_values_cover_data_mounts_and_sidecar_mounts() {
        let yaml = "data:\n  a:\n    file: a\nmounts:\n  /m:\n    file: m\n  /ref: a\nsidecars:\n  s:\n    mounts:\n      /s:\n        file: s\n";
        let mut cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.data_values_mut().len(), 3);
    }

    #[test]
    fn file_paths_are_plain_relative_paths() {
        for ok in ["run.sh", "files/run.sh", ".bashrc", "a/b/.config"] {
            assert!(validate_relative_file_path(ok).is_ok(), "rejected {}", ok);
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../x",
            "./x",
            "a//b",
            "a b",
            "x;y",
            "http://x",
            "a/",
        ] {
            assert!(
                validate_relative_file_path(bad).is_err(),
                "accepted {:?}",
                bad
            );
        }
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
        assert_eq!(resolved[0].name, "00-tools");
        let sidecar_install = cfg.sidecars.unwrap()["test"].install.clone().unwrap();
        assert_eq!(sidecar_install.resolved()[0].name, "00-tinyproxy");
    }

    #[test]
    fn all_examples_parse() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("yaml" | "yml")
                ) {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples"),
            &mut files,
        );
        assert!(!files.is_empty());
        let failures = files
            .iter()
            .filter_map(|f| {
                serde_yaml::from_str::<RoozCfg>(&std::fs::read_to_string(f).unwrap())
                    .err()
                    .map(|e| format!("{}: {}", f.display(), e))
            })
            .collect::<Vec<_>>();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
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
                    generator: Script { .. },
                    ..
                } => {
                    assert_eq!(name, "generated-file");
                }
            }
        }
    }

    #[test]
    fn a_mount_with_dir_says_what_is_wrong_with_it() {
        let parse = |y: &str| serde_yaml::from_str::<IndexMap<String, MountSource>>(y);
        let ok = parse("a: notes\nb: {dir: home, once: true}\nc: {content: x}\nd: {}\n").unwrap();
        assert!(matches!(ok["a"], MountSource::DataEntryReference(_)));
        assert!(matches!(ok["b"], MountSource::InlineDir(_)));
        assert!(matches!(
            ok["c"],
            MountSource::InlineDataValue(DataValue::InlineContent { .. })
        ));
        assert!(matches!(
            ok["d"],
            MountSource::InlineDataValue(DataValue::Dir {})
        ));
        let err = parse("b: {dir: home, skip: [x]}\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field `skip`"), "{}", err);
        let err = parse("b: {dir: home, once: maybe}\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid type"), "{}", err);
    }

    #[test]
    fn a_dir_filter_keeps_what_include_matches_and_exclude_does_not() {
        let filter = |include: Option<&[&str]>, exclude: &[&str]| {
            let strings = |g: &[&str]| g.iter().map(|g| g.to_string()).collect::<Vec<_>>();
            DirFilter::new(&DirSource {
                dir: "d".into(),
                files: None,
                executable: None,
                once: None,
                include: include.map(strings),
                exclude: Some(strings(exclude)),
            })
            .unwrap()
        };
        let all = filter(None, &[]);
        assert!(all.keeps(".bashrc") && all.keeps(".config/nvim/init.lua"));

        let some = filter(
            Some(&[".config/**", "*.sh"]),
            &["**/*.bak", ".config/secret"],
        );
        assert!(some.keeps(".config/nvim/init.lua"));
        assert!(some.keeps("run.sh"));
        // `*` stays within a directory
        assert!(!some.keeps("bin/run.sh"));
        assert!(!some.keeps(".bashrc"));
        assert!(!some.keeps(".config/nvim/init.lua.bak"));
        // a directory left out takes everything below it
        assert!(some.skips_dir(".config/secret"));
        assert!(!some.keeps(".config/secret/token"));
        assert!(!some.skips_dir(".config"));

        assert!(
            DirFilter::new(&DirSource {
                dir: "d".into(),
                files: None,
                executable: None,
                once: None,
                include: None,
                exclude: Some(vec!["a[".into()]),
            })
            .is_err()
        );
    }

    #[test]
    fn two_dirs_providing_one_path_or_an_unread_dir_are_refused() {
        let dir = |files: Vec<&str>| {
            MountSource::InlineDir(DirSource {
                dir: "d".into(),
                files: Some(
                    files
                        .into_iter()
                        .map(|p| DirFile {
                            path: p.into(),
                            content: String::new(),
                            executable: false,
                        })
                        .collect(),
                ),
                executable: None,
                once: None,
                include: None,
                exclude: None,
            })
        };
        let mut mounts = Some(IndexMap::from([
            ("~/".to_string(), dir(vec![".config/a"])),
            ("~/.config".to_string(), dir(vec!["a"])),
        ]));
        let err = expand_dir_mounts(&mut mounts).unwrap_err().to_string();
        assert!(err.contains("'~/.config/a': two 'dir:' mounts"), "{}", err);

        let mut unread = Some(IndexMap::from([(
            "~/".to_string(),
            MountSource::InlineDir(DirSource {
                dir: "d".into(),
                files: None,
                executable: None,
                once: None,
                include: None,
                exclude: None,
            }),
        )]));
        assert!(
            expand_dir_mounts(&mut unread)
                .unwrap_err()
                .to_string()
                .contains("was not read")
        );
    }
}
