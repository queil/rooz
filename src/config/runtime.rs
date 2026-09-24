use super::config::{DataValue, InstallSpec, MountSource, RoozCfg, RoozSidecar};
use crate::constants;
use crate::model::types::AnyError;
use crate::model::types::ContentGenerator;
use crate::model::types::{TargetDir, VolumeFilesSpec};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
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
    pub init: bool,
    pub work_dir: String,
    pub user: Option<String>,
    pub uid: Option<i32>,
    pub egress: bool,
    pub install: Option<InstallSpec>,
    #[serde(default)]
    pub peers: Vec<String>,
}

pub const ROOZ_META_PREFIX: &str = "ROOZ_META_";

// Env entries reach the engine as verbatim "KEY=value" strings, and the config declaring them
// can be authored by the repository being opened. A key carrying '=' or a newline would add
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

impl<'a> TryFrom<(&'a str, &'a RoozSidecar)> for RoozSidecarRuntime {
    type Error = AnyError;

    fn try_from((name, value): (&'a str, &'a RoozSidecar)) -> Result<Self, Self::Error> {
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
            privileged: value.privileged.clone().unwrap_or_default(),
            init: value.init.clone().unwrap_or(true),
            work_dir: value.work_dir.clone().unwrap_or_default(),
            user: value.user.clone(),
            egress: value.egress.clone().unwrap_or(false),
            install: value.install.clone(),
            uid: value.uid.clone(),
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
    for (_, v) in values.iter_mut() {
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
    for (_, spec) in mounts.iter_mut() {
        for file in spec.files.iter_mut() {
            mask_generator(&mut file.generator, secrets);
        }
    }
}

fn mask_data_value(value: &mut DataValue, secrets: &[&String]) {
    match value {
        DataValue::Dir {} => {}
        DataValue::InlineContent { content, .. } => mask_in(content, secrets),
        DataValue::GeneratedContent {
            generate, image, ..
        } => {
            mask_in(generate, secrets);
            mask_opt(image, secrets);
        }
    }
}

fn mask_data(data: &mut HashMap<String, DataValue>, secrets: &[&String]) {
    for (_, v) in data.iter_mut() {
        mask_data_value(v, secrets);
    }
}

fn mask_mounts(mounts: &mut HashMap<String, MountSource>, secrets: &[&String]) {
    for (_, v) in mounts.iter_mut() {
        if let MountSource::InlineDataValue(dv) = v {
            mask_data_value(dv, secrets);
        }
    }
}

fn mask_install(install: &mut Option<InstallSpec>, secrets: &[&String]) {
    match install {
        Some(InstallSpec::Script(script)) => mask_in(script, secrets),
        Some(InstallSpec::Steps(steps)) => {
            for (_, step) in steps.iter_mut() {
                if let Some(script) = step {
                    mask_in(script, secrets);
                }
            }
        }
        None => {}
    }
}

impl RuntimeConfig {
    pub fn from_string(config: String) -> Result<RuntimeConfig, AnyError> {
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

    /// A copy with every occurrence of the given secret values replaced by a marker,
    /// for persisting to the workspace-config volume. The container's environment is set
    /// at creation time from the unmasked values, so behaviour is unchanged.
    ///
    /// `shell` is deliberately left alone on both the workspace and its sidecars: `rooz
    /// enter` executes it. Nothing else in the persisted file is read back for its value.
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

        for (_, sidecar) in masked.sidecars.iter_mut() {
            mask_in(&mut sidecar.image, &secrets);
            mask_map(&mut sidecar.env, &secrets);
            mask_vec(&mut sidecar.command, &secrets);
            mask_vec(&mut sidecar.args, &secrets);
            mask_vec(&mut sidecar.ports, &secrets);
            mask_in(&mut sidecar.work_dir, &secrets);
            mask_opt(&mut sidecar.user, &secrets);
            mask_mounts(&mut sidecar.mounts, &secrets);
            mask_real_mounts(&mut sidecar.real_mounts, &secrets);
            mask_install(&mut sidecar.install, &secrets);
        }

        masked
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

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(yaml: &str) -> RuntimeConfig {
        let cfg: RoozCfg = serde_yaml::from_str(yaml).unwrap();
        RuntimeConfig::try_from(&cfg).unwrap()
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
        steps.insert("10-a".to_string(), Some("echo a".to_string()));
        steps.insert("20-b".to_string(), None);
        let cfg = RuntimeConfig {
            install: Some(InstallSpec::Steps(steps.clone())),
            ..Default::default()
        };
        let parsed = RuntimeConfig::from_string(cfg.to_string().unwrap()).unwrap();
        assert_eq!(parsed.install, Some(InstallSpec::Steps(steps)));
    }
}

impl<'a> TryFrom<&'a RoozCfg> for RuntimeConfig {
    type Error = AnyError;

    fn try_from(value: &'a RoozCfg) -> Result<Self, Self::Error> {
        let default = RuntimeConfig::default();

        let sidecar_cfgs = value.sidecars.clone().unwrap_or_default();
        for (name, s) in &sidecar_cfgs {
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

        Ok(RuntimeConfig {
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
        })
    }
}
