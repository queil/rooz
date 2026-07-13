use super::config::{DataValue, InstallSpec, MountSource, RoozCfg, RoozSidecar};
use crate::constants;
use crate::model::types::AnyError;
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
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
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

    pub fn all_mounts(&self) -> HashMap<(String, String), MountSource> {
        self.mounts
            .iter()
            .map(|(target, source)| (("main".to_string(), target.clone()), source.clone()))
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
        assert!(err.contains("'image' is required"), "unexpected error: {}", err);
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

        let mut ports = HashMap::<String, Option<String>>::new();
        RoozCfg::parse_ports(&mut ports, value.clone().ports.unwrap_or_default());

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
            sidecars: value
                .sidecars
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| Ok((k.clone(), (k.as_str(), &v).try_into()?)))
                .collect::<Result<HashMap<_, _>, AnyError>>()?,
            env: value.env.clone().unwrap_or_default().into_iter().collect(),
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
