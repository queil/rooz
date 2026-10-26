use super::config::{DataValue, InstallSpec, MountSource, RoozCfg, RoozSidecar};
use crate::constants;
use crate::model::types::AnyError;
use crate::model::types::ContentGenerator;
use crate::model::types::{TargetDir, VolumeFilesSpec};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
#[serde_with::skip_serializing_none]
pub struct RoozSidecarRuntime {
    pub image: String,
    pub env: HashMap<String, String>,
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub shell: Option<Vec<String>>,
    pub mounts: HashMap<String, MountSource>,
    pub real_mounts: HashMap<TargetDir, VolumeFilesSpec>,
    pub ports: Vec<String>,
    pub privileged: bool,
    #[serde(default)]
    pub vpn: bool,
    pub init: bool,
    pub work_dir: String,
    pub uid: Option<i32>,
    pub egress: bool,
    pub install: Option<InstallSpec>,
    #[serde(default)]
    pub peers: Vec<String>,
}

pub const ROOZ_META_PREFIX: &str = "ROOZ_META_";

// Env entries reach the engine as verbatim "KEY=value" strings, and the config declaring them
// can come from a remote config. A key carrying '=' or a newline would add
// assignments of its own, and one spelling a ROOZ_META_* name would displace the metadata rooz
// injects and the operator's tooling reads back.
pub fn validate_env_key(key: &str, origin: &str) -> Result<(), AnyError> {
    let shaped = !key.is_empty()
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');

    if !shaped {
        return Err(format!(
            "{}: env key '{}' is not a valid variable name - letters, digits and underscores \
             only, not starting with a digit",
            origin, key
        )
        .into());
    }
    if key.starts_with(ROOZ_META_PREFIX) {
        return Err(format!(
            "{}: env key '{}' is refused - the {}* names are rooz's own metadata",
            origin, key, ROOZ_META_PREFIX
        )
        .into());
    }
    Ok(())
}

// The workspace user is interpolated into '/home/{user}', which decides where the global
// ssh key volume lands and how '~/' mount targets expand. The config declaring it can come
// from a remote config, so only a plain account name is acceptable:
// anything carrying path separators or '..' relocates those mounts elsewhere in the container.
pub fn validate_user(user: &str) -> Result<(), AnyError> {
    let shaped = !user.is_empty()
        && user
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !user.contains("..");

    if !shaped {
        return Err(format!(
            "user '{}' is refused: it must be a plain account name - letters, digits, \
             '_', '-' and '.' only, not starting with '-' or '.'",
            user
        )
        .into());
    }
    Ok(())
}

// A mount target names where a mount lands, and rooz hands its parents in the home dir to the
// user as root: one with '.' or '..' segments could name a directory anywhere in the container.
pub fn validate_mount_target(container: &str, target: &str) -> Result<(), AnyError> {
    if target.split('/').any(|s| s == "." || s == "..") {
        return Err(format!(
            "{}: mount path '{}' is refused - '.' and '..' segments are not allowed",
            container, target
        )
        .into());
    }
    Ok(())
}

impl<'a> TryFrom<(&'a str, &'a RoozSidecar)> for RoozSidecarRuntime {
    type Error = AnyError;

    fn try_from((name, value): (&'a str, &'a RoozSidecar)) -> Result<Self, Self::Error> {
        let vpn = value.vpn.unwrap_or_default();
        if vpn && value.uid.is_some_and(|uid| uid != constants::ROOT_UID_INT) {
            return Err(format!(
                "sidecar '{}': 'vpn' requires the sidecar to run as uid 0 - NET_ADMIN is not \
                 effective for a non-root user",
                name
            )
            .into());
        }
        Ok(RoozSidecarRuntime {
            image: value.image.clone().ok_or_else(|| -> AnyError {
                format!(
                    "sidecar '{}': 'image' is required after merging all config layers",
                    name
                )
                .into()
            })?,
            env: value
                .env
                .clone()
                .unwrap_or_default()
                .iter()
                .map(|(k, v)| {
                    validate_env_key(k, &format!("sidecar '{}'", name))?;
                    Ok((k.to_string(), v.to_string()))
                })
                .collect::<Result<HashMap<_, _>, AnyError>>()?,
            command: value.command.clone().unwrap_or_default(),
            args: value.args.clone().unwrap_or_default(),
            shell: value.shell.clone(),
            mounts: value
                .mounts
                .clone()
                .unwrap_or_default()
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            real_mounts: HashMap::new(),
            ports: value.ports.clone().unwrap_or_default(),
            privileged: value.privileged.unwrap_or_default(),
            vpn,
            init: value.init.unwrap_or(true),
            work_dir: value.work_dir.clone().unwrap_or_default(),
            // a policy still here was never expanded into its proxy: no way out at all
            egress: value.egress.as_ref().is_some_and(|e| e.is_open()),
            install: value.install.clone(),
            uid: value.uid,
            peers: value.peers.clone().unwrap_or_default(),
        })
    }
}
#[serde_with::skip_serializing_none]
#[serde_as]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RuntimeConfig {
    pub git_ssh_url: Option<String>,
    pub extra_repos: Vec<String>,
    pub image: String,
    pub caches: Vec<String>,
    pub shell: Vec<String>,
    pub user: String,
    pub uid: i32,
    pub ports: HashMap<String, Option<String>>,
    pub privileged: bool,
    pub init: bool,
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub sidecars: HashMap<String, RoozSidecarRuntime>,
    pub data: HashMap<String, DataValue>,
    pub mounts: HashMap<String, MountSource>,
    pub real_mounts: HashMap<TargetDir, VolumeFilesSpec>,
    pub install: Option<InstallSpec>,
    pub egress: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            git_ssh_url: None,
            extra_repos: Vec::new(),
            image: constants::DEFAULT_IMAGE.into(),
            caches: Vec::new(),
            shell: vec![constants::DEFAULT_SHELL.into()],
            user: constants::DEFAULT_USER.into(),
            uid: constants::DEFAULT_UID.parse().unwrap(),
            ports: HashMap::new(),
            privileged: false,
            init: true,
            command: Vec::new(),
            args: Vec::new(),
            sidecars: HashMap::new(),
            env: HashMap::new(),
            data: HashMap::new(),
            mounts: HashMap::new(),
            real_mounts: HashMap::new(),
            install: None,
            egress: true,
        }
    }
}

pub const SECRET_MASK: &str = "***";

fn mask_in(value: &mut String, secrets: &[&String]) {
    // longest first: masking a secret that is a part of another would leave the rest readable
    let mut secrets = secrets.to_vec();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in secrets {
        if value.contains(secret.as_str()) {
            *value = value.replace(secret.as_str(), SECRET_MASK);
        }
    }
}

fn mask_opt(value: &mut Option<String>, secrets: &[&String]) {
    if let Some(v) = value {
        mask_in(v, secrets);
    }
}

fn mask_vec(values: &mut [String], secrets: &[&String]) {
    for v in values.iter_mut() {
        mask_in(v, secrets);
    }
}

fn mask_map(values: &mut HashMap<String, String>, secrets: &[&String]) {
    for v in values.values_mut() {
        mask_in(v, secrets);
    }
}

fn mask_generator(generator: &mut ContentGenerator, secrets: &[&String]) {
    match generator {
        ContentGenerator::Inline(content) => mask_in(content, secrets),
        ContentGenerator::Script { script, image } => {
            mask_in(script, secrets);
            mask_opt(image, secrets);
        }
    }
}

fn mask_real_mounts(mounts: &mut HashMap<TargetDir, VolumeFilesSpec>, secrets: &[&String]) {
    for spec in mounts.values_mut() {
        for file in spec.files.iter_mut() {
            mask_generator(&mut file.generator, secrets);
        }
    }
}

fn mask_data_value(value: &mut DataValue, secrets: &[&String]) {
    match value {
        DataValue::Dir {} => {}
        DataValue::InlineContent { content, .. } => mask_in(content, secrets),
        DataValue::FileContent { content, .. } => {
            if let Some(content) = content {
                mask_in(content, secrets)
            }
        }
        DataValue::GeneratedContent {
            generate, image, ..
        } => {
            mask_in(generate, secrets);
            mask_opt(image, secrets);
        }
    }
}

fn mask_data(data: &mut HashMap<String, DataValue>, secrets: &[&String]) {
    for v in data.values_mut() {
        mask_data_value(v, secrets);
    }
}

fn mask_mounts(mounts: &mut HashMap<String, MountSource>, secrets: &[&String]) {
    for v in mounts.values_mut() {
        if let MountSource::InlineDataValue(dv) = v {
            mask_data_value(dv, secrets);
        }
    }
}

fn mask_install(install: &mut Option<InstallSpec>, secrets: &[&String]) {
    match install {
        Some(InstallSpec::Script(script)) => mask_in(script, secrets),
        Some(InstallSpec::Steps(steps)) => {
            for step in steps.values_mut().flatten() {
                for text in step.texts_mut() {
                    mask_in(text, secrets);
                }
            }
        }
        None => {}
    }
}

impl RuntimeConfig {
    pub fn from_string(config: String) -> Result<RuntimeConfig, AnyError> {
        if config.trim().is_empty() {
            return Err(
                "The workspace has no stored runtime config: its creation did not finish. \
                Run 'rooz update' to finish it, or remove it and run 'rooz new' again."
                    .into(),
            );
        }
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

    pub fn workspace_networks(
        sidecars: &HashMap<String, RoozSidecarRuntime>,
    ) -> (Vec<String>, Vec<(String, String)>) {
        let mut pairs: Vec<String> = sidecars.keys().cloned().collect();
        pairs.sort();
        let mut peers = sidecars
            .iter()
            .flat_map(|(name, s)| {
                s.peers.iter().map(move |p| {
                    if name < p {
                        (name.clone(), p.clone())
                    } else {
                        (p.clone(), name.clone())
                    }
                })
            })
            .collect::<Vec<_>>();
        peers.sort();
        peers.dedup();
        (pairs, peers)
    }

    /// Names of the containers this config would create with host-level privileges.
    pub fn privileged_containers(&self) -> Vec<String> {
        let mut names = Vec::new();
        if self.privileged {
            names.push(constants::DEFAULT_CONTAINER_NAME.to_string());
        }
        names.extend(
            self.sidecars
                .iter()
                .filter(|(_, s)| s.privileged)
                .map(|(name, _)| name.clone()),
        );
        names.sort();
        names
    }

    /// A fingerprint of everything that decides what the named container runs and reaches: its
    /// image, command, env, uid, install steps, mounts and the data behind them. Consent to a
    /// privileged or vpn container is pinned to it, so a config that redefines a consented name
    /// is refused until the operator consents to the new definition. Call it on a copy with the
    /// secrets masked: a rotated secret keeps the fingerprint, and no plaintext gets hashed.
    pub fn fingerprint(&self, container: &str) -> String {
        let definition = match self.sidecars.get(container) {
            Some(sidecar) if container != constants::DEFAULT_CONTAINER_NAME => {
                let mut sidecar = sidecar.clone();
                sidecar.real_mounts.clear();
                let data = sidecar
                    .mounts
                    .values()
                    .filter_map(|m| match m {
                        MountSource::DataEntryReference(key) => self
                            .data
                            .get(key.as_str())
                            .map(|v| (key.as_str().to_string(), v.clone())),
                        MountSource::InlineDataValue(_) | MountSource::InlineDir(_) => None,
                    })
                    .collect::<std::collections::BTreeMap<_, _>>();
                serde_json::to_value((sidecar, data))
            }
            _ => {
                let mut work = self.clone();
                work.sidecars.clear();
                work.real_mounts.clear();
                serde_json::to_value(work)
            }
        }
        .expect("a runtime config serializes");
        // serde_json's maps are sorted, so the text is the same for the same definition
        let digest = Sha256::digest(definition.to_string());
        format!("{:x}", digest)[..16].to_string()
    }

    pub fn vpn_containers(&self) -> Vec<String> {
        let mut names = self
            .sidecars
            .iter()
            .filter(|(_, s)| s.vpn)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    /// A copy with every occurrence of the given secret values replaced by a marker,
    /// for persisting to the workspace-config volume. The container's environment is set
    /// at creation time from the unmasked values, so behaviour is unchanged.
    ///
    /// `shell` is deliberately left alone on both the workspace and its sidecars: `rooz
    /// enter` executes it. `git_ssh_url` is read back by `rooz update`, which skips a masked one.
    /// Nothing else in the persisted file is read back for its value.
    pub fn mask_secrets(&self, secret_values: &[String]) -> Self {
        let secrets = secret_values
            .iter()
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>();

        let mut masked = self.clone();
        if secrets.is_empty() {
            return masked;
        }

        mask_opt(&mut masked.git_ssh_url, &secrets);
        mask_vec(&mut masked.extra_repos, &secrets);
        mask_in(&mut masked.image, &secrets);
        mask_vec(&mut masked.caches, &secrets);
        mask_in(&mut masked.user, &secrets);
        mask_vec(&mut masked.command, &secrets);
        mask_vec(&mut masked.args, &secrets);
        mask_map(&mut masked.env, &secrets);
        mask_data(&mut masked.data, &secrets);
        mask_mounts(&mut masked.mounts, &secrets);
        mask_real_mounts(&mut masked.real_mounts, &secrets);
        mask_install(&mut masked.install, &secrets);

        for sidecar in masked.sidecars.values_mut() {
            mask_in(&mut sidecar.image, &secrets);
            mask_map(&mut sidecar.env, &secrets);
            mask_vec(&mut sidecar.command, &secrets);
            mask_vec(&mut sidecar.args, &secrets);
            mask_vec(&mut sidecar.ports, &secrets);
            mask_in(&mut sidecar.work_dir, &secrets);
            mask_mounts(&mut sidecar.mounts, &secrets);
            mask_real_mounts(&mut sidecar.real_mounts, &secrets);
            mask_install(&mut sidecar.install, &secrets);
        }

        masked
    }

    // Volumes and inline files are named after data keys and mount paths, sanitized: two that
    // sanitize alike would share a volume or a file, and an 'inline' key the inline files' volume.
    fn validate_data_names(&self) -> Result<(), AnyError> {
        let mut volumes = HashMap::<String, &str>::new();
        for key in self.data.keys() {
            let derived = crate::util::id::sanitize(key);
            if derived == "inline" {
                return Err(format!(
                    "data key '{}' is reserved: it names the volume holding inline mount files",
                    key
                )
                .into());
            }
            if derived.starts_with('-') {
                return Err(format!("data key '{}' must start with a letter or digit", key).into());
            }
            if let Some(other) = volumes.insert(derived.clone(), key) {
                return Err(format!(
                    "data keys '{}' and '{}' would share volume '{}': rename one",
                    other, key, derived
                )
                .into());
            }
        }
        let mut files = HashMap::<String, (String, String)>::new();
        for ((container, target), source) in self.all_mounts() {
            if let MountSource::InlineDataValue(_) = source {
                let key = source.resolve_key(&container, &target);
                if let Some((_, other)) = files.insert(key, (container.clone(), target.clone())) {
                    return Err(format!(
                        "{}: mounts '{}' and '{}' would share one inline file: rename one",
                        container, other, target
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    pub fn all_mounts(&self) -> HashMap<(String, String), MountSource> {
        self.mounts
            .iter()
            .map(|(target, source)| {
                (
                    (
                        constants::DEFAULT_CONTAINER_NAME.to_string(),
                        target.clone(),
                    ),
                    source.clone(),
                )
            })
            .chain(self.sidecars.iter().flat_map(|(sidecar_name, sidecar)| {
                sidecar.mounts.iter().map(|(target, source)| {
                    ((sidecar_name.clone(), target.clone()), source.clone())
                })
            }))
            .collect()
    }
}

impl<'a> TryFrom<&'a RoozCfg> for RuntimeConfig {
    type Error = AnyError;

    fn try_from(value: &'a RoozCfg) -> Result<Self, Self::Error> {
        let default = RuntimeConfig::default();

        let sidecar_cfgs = value.sidecars.clone().unwrap_or_default();
        for (name, s) in &sidecar_cfgs {
            crate::util::id::validate_sidecar_name(name)?;
            for peer in s.peers.iter().flatten() {
                if peer == name {
                    return Err(
                        format!("sidecar '{}': cannot declare itself as a peer", name).into(),
                    );
                }
                if !sidecar_cfgs.contains_key(peer) {
                    return Err(format!(
                        "sidecar '{}': unknown peer '{}' (peers must name sidecars defined in this workspace)",
                        name, peer
                    )
                    .into());
                }
            }
        }

        let mut ports = HashMap::<String, Option<String>>::new();
        RoozCfg::parse_ports(&mut ports, value.clone().ports.unwrap_or_default())?;

        let user = value.user.as_deref().unwrap_or(&default.user);
        validate_user(user)?;
        for target in value.mounts.iter().flat_map(|m| m.keys()) {
            validate_mount_target(constants::DEFAULT_CONTAINER_NAME, target)?;
        }
        for (name, s) in value.sidecars.iter().flatten() {
            for target in s.mounts.iter().flat_map(|m| m.keys()) {
                validate_mount_target(name, target)?;
            }
        }

        let cfg = RuntimeConfig {
            git_ssh_url: value.git_ssh_url.clone(),
            extra_repos: value
                .extra_repos
                .as_deref()
                .unwrap_or(&default.extra_repos)
                .to_vec(),
            shell: value.shell.as_deref().unwrap_or(&default.shell).into(),
            image: value.image.as_deref().unwrap_or(&default.image).into(),
            user: user.to_string(),
            caches: {
                let mut seen = std::collections::HashSet::new();
                let mut val = value.caches.as_deref().unwrap_or(&default.caches).to_vec();
                val.retain(|c| seen.insert(c.clone()));
                val
            },
            sidecars: sidecar_cfgs
                .into_iter()
                .map(|(k, v)| Ok((k.clone(), (k.as_str(), &v).try_into()?)))
                .collect::<Result<HashMap<_, _>, AnyError>>()?,
            env: value
                .env
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| {
                    validate_env_key(&k, "workspace")?;
                    Ok((k, v))
                })
                .collect::<Result<HashMap<_, _>, AnyError>>()?,
            ports,
            privileged: value.privileged.unwrap_or(default.privileged),
            init: value.init.unwrap_or(default.init),
            command: value
                .command
                .as_deref()
                .unwrap_or(&default.command)
                .to_vec(),
            args: value.args.as_deref().unwrap_or(&default.args).to_vec(),
            data: value.data.clone().unwrap_or_default().into_iter().collect(),
            mounts: value
                .mounts
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            install: value.install.clone(),
            ..default
        };
        cfg.validate_data_names()?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn masking_a_secret_inside_another_leaves_nothing_readable() {
        let (short, long) = ("abc".to_string(), "abcdef".to_string());
        for secrets in [[&short, &long], [&long, &short]] {
            let mut value = "token=abcdef".to_string();
            super::mask_in(&mut value, &secrets);
            assert_eq!(value, "token=***");
        }
    }

    use super::*;

    fn try_runtime(yaml: &str) -> Result<RuntimeConfig, AnyError> {
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        RuntimeConfig::try_from(&cfg)
    }

    #[test]
    fn sidecar_names_keep_to_the_workspace_alphabet() {
        assert!(try_runtime("sidecars:\n  db-1:\n    image: a\n").is_ok());
        for bad in ["work", "\"my db\"", "My", "db_1", "-db"] {
            let err = try_runtime(&format!("sidecars:\n  {}:\n    image: a\n", bad))
                .unwrap_err()
                .to_string();
            assert!(err.contains("sidecar name"), "{}: {}", bad, err);
        }
    }

    #[test]
    fn data_keys_that_would_share_a_volume_are_refused() {
        for (yaml, needle) in [
            ("data:\n  Notes: {}\n  notes: {}\n", "would share volume"),
            ("data:\n  a.b: {}\n  a-b: {}\n", "would share volume"),
            ("data:\n  inline: {}\n", "reserved"),
            ("data:\n  ~x: {}\n", "letter or digit"),
        ] {
            let err = try_runtime(yaml).unwrap_err().to_string();
            assert!(err.contains(needle), "{}: {}", yaml, err);
        }
        assert!(try_runtime("data:\n  install-dotnet.weir: {}\n  git-config: {}\n").is_ok());
    }

    #[test]
    fn inline_files_are_one_per_container_and_path() {
        // the same path in the work container and a sidecar: two files
        let cfg = try_runtime(
            "mounts:\n  /etc/x:\n    content: work\nsidecars:\n  db:\n    image: a\n    mounts:\n      /etc/x:\n        content: db\n",
        )
        .unwrap();
        let specs =
            crate::api::VolumeApi::create_volume_specs("ws", &cfg.data, &cfg.all_mounts(), true)
                .unwrap();
        assert!(specs.contains_key(&crate::model::types::DataEntryKey("-etc-x".into())));
        assert!(specs.contains_key(&crate::model::types::DataEntryKey("_db_-etc-x".into())));
        // paths that sanitize alike in one container: refused
        let err = try_runtime(
            "mounts:\n  /etc/a.conf:\n    content: a\n  /etc/a-conf:\n    content: b\n",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("would share one inline file"), "{}", err);
    }

    #[test]
    fn empty_stored_config_is_named_as_unfinished_workspace() {
        let err = RuntimeConfig::from_string(String::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("creation did not finish"), "{}", err);
    }

    #[test]
    fn vpn_sidecar_must_run_as_root() {
        let ok: RoozCfg =
            serde_yaml::from_str("sidecars:\n  wg:\n    image: a\n    vpn: true\n").unwrap();
        assert!(RuntimeConfig::try_from(&ok).unwrap().sidecars["wg"].vpn);
        let root: RoozCfg =
            serde_yaml::from_str("sidecars:\n  wg:\n    image: a\n    vpn: true\n    uid: 0\n")
                .unwrap();
        assert!(RuntimeConfig::try_from(&root).is_ok());
        let non_root: RoozCfg =
            serde_yaml::from_str("sidecars:\n  wg:\n    image: a\n    vpn: true\n    uid: 1000\n")
                .unwrap();
        let err = RuntimeConfig::try_from(&non_root).unwrap_err().to_string();
        assert!(err.contains("wg") && err.contains("uid 0"), "{}", err);
    }

    #[test]
    fn old_persisted_sidecar_without_vpn_parses() {
        let yaml = "sidecars:\n  svc:\n    image: alpine\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        let runtime = RuntimeConfig::try_from(&cfg).unwrap();
        let persisted = runtime
            .to_string()
            .unwrap()
            .lines()
            .filter(|l| !l.contains("vpn"))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed = RuntimeConfig::from_string(persisted).unwrap();
        assert!(!parsed.sidecars["svc"].vpn);
    }

    fn runtime(yaml: &str) -> RuntimeConfig {
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        RuntimeConfig::try_from(&cfg).unwrap()
    }

    #[test]
    fn hostile_users_are_refused() {
        // the user lands in '/home/{user}', which is where the ssh key volume mounts
        for hostile in ["..", ".", "../../tmp", "a/b", "", "-x", ".hidden", "a b"] {
            let cfg = RoozCfg {
                image: Some("alpine".to_string()),
                user: Some(hostile.to_string()),
                ..RoozCfg::none()
            };
            assert!(
                RuntimeConfig::try_from(&cfg).is_err(),
                "accepted user: {:?}",
                hostile
            );
        }
    }

    #[test]
    fn plain_account_names_pass() {
        for user in ["rooz_user", "root", "a-b.c", "_svc", "user1"] {
            assert!(validate_user(user).is_ok(), "rejected user: {}", user);
        }
    }

    #[test]
    fn hostile_env_keys_are_refused() {
        // keys are concatenated into "KEY=value" for the engine, so '=' and newlines would
        // smuggle in assignments of their own
        for hostile in ["LD_PRELOAD=/x.so", "A\nB", "2FOO", "", "WITH SPACE"] {
            let cfg = RoozCfg {
                image: Some("alpine".to_string()),
                env: Some(indexmap::IndexMap::from_iter([(
                    hostile.to_string(),
                    "v".to_string(),
                )])),
                ..RoozCfg::none()
            };
            assert!(
                RuntimeConfig::try_from(&cfg).is_err(),
                "accepted env key: {:?}",
                hostile
            );
        }
    }

    #[test]
    fn the_rooz_meta_namespace_is_reserved() {
        let cfg: RoozCfg =
            serde_yaml::from_str("image: alpine\nenv:\n  ROOZ_META_IMAGE: fake-image\n").unwrap();
        let err = RuntimeConfig::try_from(&cfg).unwrap_err().to_string();
        assert!(err.contains("ROOZ_META_"), "{}", err);

        // and a sidecar cannot do it either
        let cfg: RoozCfg = serde_yaml::from_str(
            "image: alpine\nsidecars:\n  svc:\n    image: alpine\n    env:\n      ROOZ_META_USER: root\n",
        )
        .unwrap();
        let err = RuntimeConfig::try_from(&cfg).unwrap_err().to_string();
        assert!(err.contains("svc"), "sidecar not named: {}", err);
    }

    #[test]
    fn ordinary_env_keys_still_pass() {
        let cfg = runtime("image: alpine\nenv:\n  API_TOKEN: t\n  _private: x\n");
        assert_eq!(cfg.env.get("API_TOKEN"), Some(&"t".to_string()));
        assert_eq!(cfg.env.get("_private"), Some(&"x".to_string()));
    }

    #[test]
    fn masks_secrets_everywhere_they_can_land() {
        let secret = "e2e-secret-MARKER-7f3a";
        let yaml = format!(
            concat!(
                "image: alpine\n",
                "user: rooz_user\n",
                "command: [\"sh\", \"-c\", \"echo {s}\"]\n",
                "args: [\"--token={s}\"]\n",
                "git_ssh_url: \"https://x:{s}@host/repo.git\"\n",
                "install: \"export T={s}\"\n",
                "env:\n  LEAK: \"{s}\"\n",
                "data:\n",
                "  secretfile:\n    content: \"leak={s}\"\n",
                "  gen:\n    generate: \"echo {s}\"\n",
                "sidecars:\n",
                "  db:\n    image: postgres\n    env:\n      PW: \"{s}\"\n    args: [\"--pw={s}\"]\n",
            ),
            s = secret
        );
        let cfg = runtime(&yaml);
        let masked = cfg.mask_secrets(&[secret.to_string()]);
        let persisted = masked.to_string().unwrap();

        assert!(
            !persisted.contains(secret),
            "secret survived masking:\n{}",
            persisted
        );
        assert!(
            persisted.contains(SECRET_MASK),
            "nothing masked:\n{}",
            persisted
        );
        // the unmasked config is what the container is created from
        assert_eq!(cfg.env["LEAK"], secret);
        assert_eq!(cfg.sidecars["db"].env["PW"], secret);
    }

    #[test]
    fn masks_inside_larger_strings_not_just_whole_values() {
        let cfg = runtime("image: alpine\nenv:\n  URL: \"https://u:hunter2@host/p\"\n");
        let masked = cfg.mask_secrets(&["hunter2".to_string()]);
        assert_eq!(masked.env["URL"], "https://u:***@host/p");
    }

    #[test]
    fn shell_is_left_alone_so_enter_keeps_working() {
        let cfg = runtime("image: alpine\nshell: [\"/bin/bash\"]\nenv:\n  A: bash\n");
        let masked = cfg.mask_secrets(&["bash".to_string()]);
        assert_eq!(masked.shell, vec!["/bin/bash".to_string()]);
        assert_eq!(masked.env["A"], SECRET_MASK);
    }

    #[test]
    fn masking_is_a_no_op_without_secrets() {
        let cfg = runtime("image: alpine\nenv:\n  A: b\n");
        assert_eq!(
            cfg.mask_secrets(&[]).to_string().unwrap(),
            cfg.to_string().unwrap()
        );
        // an empty secret value must not mask every character
        assert_eq!(
            cfg.mask_secrets(&["".to_string()]).to_string().unwrap(),
            cfg.to_string().unwrap()
        );
    }

    #[test]
    fn old_persisted_string_install_still_parses() {
        let mut yaml = RuntimeConfig::default().to_string().unwrap();
        yaml.push_str("install: apk add jq\n");
        let parsed = RuntimeConfig::from_string(yaml).unwrap();
        assert!(matches!(
            parsed.install,
            Some(InstallSpec::Script(s)) if s == "apk add jq"
        ));
    }

    #[test]
    fn sidecar_with_image_converts() {
        let yaml = "sidecars:\n  svc:\n    image: alpine\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        let runtime = RuntimeConfig::try_from(&cfg).unwrap();
        assert_eq!(runtime.sidecars["svc"].image, "alpine");
    }

    #[test]
    fn sidecar_without_image_fails_conversion_naming_sidecar() {
        let yaml = "sidecars:\n  svc:\n    env:\n      A: b\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        let err = RuntimeConfig::try_from(&cfg).unwrap_err().to_string();
        assert!(err.contains("sidecar 'svc'"), "unexpected error: {}", err);
        assert!(
            err.contains("'image' is required"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn unknown_peer_fails_conversion() {
        let yaml = "sidecars:\n  dkr:\n    image: a\n    peers: [bogus]\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        let err = RuntimeConfig::try_from(&cfg).unwrap_err().to_string();
        assert!(err.contains("sidecar 'dkr'"), "unexpected error: {}", err);
        assert!(
            err.contains("unknown peer 'bogus'"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn self_peer_fails_conversion() {
        let yaml = "sidecars:\n  dkr:\n    image: a\n    peers: [dkr]\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        let err = RuntimeConfig::try_from(&cfg).unwrap_err().to_string();
        assert!(
            err.contains("sidecar 'dkr'") && err.contains("itself"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn a_mount_path_with_dot_segments_is_refused() {
        for (yaml, container) in [
            ("mounts:\n  ~/../etc/x:\n    content: x\n", "work"),
            (
                "sidecars:\n  s:\n    image: a\n    mounts:\n      /a/./b: {}\n",
                "s",
            ),
        ] {
            let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
            let err = RuntimeConfig::try_from(&cfg).unwrap_err().to_string();
            assert!(
                err.starts_with(&format!("{}: mount path", container)),
                "{}",
                err
            );
        }
        let ok: RoozCfg =
            serde_yaml::from_str("mounts:\n  ~/.config/..x:\n    content: x\n").unwrap();
        assert!(RuntimeConfig::try_from(&ok).is_ok());
    }

    #[test]
    fn old_persisted_sidecar_without_peers_parses() {
        let yaml = "sidecars:\n  svc:\n    image: alpine\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        let runtime = RuntimeConfig::try_from(&cfg).unwrap();
        let mut persisted = runtime.to_string().unwrap();
        persisted = persisted
            .lines()
            .filter(|l| !l.contains("peers"))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed = RuntimeConfig::from_string(persisted).unwrap();
        assert!(parsed.sidecars["svc"].peers.is_empty());
    }

    #[test]
    fn workspace_networks_no_peers() {
        let yaml = "sidecars:\n  a:\n    image: x\n  b:\n    image: x\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        let runtime = RuntimeConfig::try_from(&cfg).unwrap();
        let (pairs, peers) = RuntimeConfig::workspace_networks(&runtime.sidecars);
        assert_eq!(pairs, vec!["a".to_string(), "b".to_string()]);
        assert!(peers.is_empty());
    }

    #[test]
    fn workspace_networks_peer_dedup_bidirectional() {
        let yaml =
            "sidecars:\n  a:\n    image: x\n    peers: [b]\n  b:\n    image: x\n    peers: [a]\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        let runtime = RuntimeConfig::try_from(&cfg).unwrap();
        let (_, peers) = RuntimeConfig::workspace_networks(&runtime.sidecars);
        assert_eq!(peers, vec![("a".to_string(), "b".to_string())]);
    }

    #[test]
    fn workspace_networks_example_topology() {
        let yaml = "sidecars:\n  claude:\n    image: x\n    peers: [proxy]\n  proxy:\n    image: x\n    egress: true\n  dkr:\n    image: x\n    peers: [images]\n  images:\n    image: x\n    egress: true\n";
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        let runtime = RuntimeConfig::try_from(&cfg).unwrap();
        let (pairs, peers) = RuntimeConfig::workspace_networks(&runtime.sidecars);
        assert_eq!(
            pairs,
            vec![
                "claude".to_string(),
                "dkr".to_string(),
                "images".to_string(),
                "proxy".to_string()
            ]
        );
        assert_eq!(
            peers,
            vec![
                ("claude".to_string(), "proxy".to_string()),
                ("dkr".to_string(), "images".to_string())
            ]
        );
    }

    #[test]
    fn step_map_install_roundtrips() {
        let mut steps = indexmap::IndexMap::new();
        steps.insert("10-a".to_string(), Some("echo a".into()));
        steps.insert("20-b".to_string(), None);
        let cfg = RuntimeConfig {
            install: Some(InstallSpec::Steps(steps.clone())),
            ..Default::default()
        };
        let parsed = RuntimeConfig::from_string(cfg.to_string().unwrap()).unwrap();
        assert_eq!(parsed.install, Some(InstallSpec::Steps(steps)));
    }
}
