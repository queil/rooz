use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::model::types::AnyError;

pub const REMOTES_FILE_ENV: &str = "ROOZ_REMOTES_FILE";
const RESERVED_NAMES: [&str; 3] = ["add", "rm", "ls"];
const MAX_NAME_LEN: usize = 32;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub ssh: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Remotes {
    #[serde(default)]
    pub hosts: BTreeMap<String, Host>,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
}

fn home() -> Result<PathBuf, AnyError> {
    env_path("HOME").ok_or_else(|| "HOME is not set".into())
}

pub fn file_path() -> Result<PathBuf, AnyError> {
    if let Some(path) = env_path(REMOTES_FILE_ENV) {
        return Ok(path);
    }
    let config_dir = match env_path("XDG_CONFIG_HOME") {
        Some(dir) => dir,
        None => home()?.join(".config"),
    };
    Ok(config_dir.join("rooz").join("remotes.yaml"))
}

pub fn socket_dir() -> Result<PathBuf, AnyError> {
    Ok(match env_path("XDG_RUNTIME_DIR") {
        Some(dir) => dir.join("rooz"),
        None => home()?.join(".rooz"),
    })
}

pub fn socket_path(name: &str) -> Result<PathBuf, AnyError> {
    Ok(socket_dir()?.join(format!("{}.sock", name)))
}

pub fn validate_name(name: &str) -> Result<(), AnyError> {
    let valid_chars = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if name.is_empty() || name.len() > MAX_NAME_LEN || !valid_chars || name.starts_with(['-', '_'])
    {
        return Err(format!(
            "invalid host name '{}': 1-{} chars, lowercase letters, digits, '-' and '_', not starting with '-' or '_'",
            name, MAX_NAME_LEN
        )
        .into());
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(format!("'{}' is reserved, pick another host name", name).into());
    }
    Ok(())
}

pub fn is_connected(socket_path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(socket_path).is_ok()
}

// The local socket 'rooz remote' forwards for the named host
pub fn connected_socket(name: &str) -> Result<PathBuf, AnyError> {
    Remotes::load()?.get(name)?;
    let socket = socket_path(name)?;
    if !is_connected(&socket) {
        return Err(format!(
            "Host '{}' is not connected. Run 'rooz remote {}' (it stays in the foreground)",
            name, name
        )
        .into());
    }
    Ok(socket)
}

impl Remotes {
    pub fn parse(yaml: &str) -> Result<Self, AnyError> {
        let remotes: Remotes = serde_yaml::from_str(yaml)?;
        for name in remotes.hosts.keys() {
            validate_name(name)?;
        }
        Ok(remotes)
    }

    pub fn load_from(path: &Path) -> Result<Self, AnyError> {
        match std::fs::read_to_string(path) {
            Ok(yaml) => Self::parse(&yaml).map_err(|e| format!("{}: {}", path.display(), e).into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("Could not read {}: {}", path.display(), e).into()),
        }
    }

    pub fn load() -> Result<Self, AnyError> {
        Self::load_from(&file_path()?)
    }

    pub fn save(&self) -> Result<PathBuf, AnyError> {
        let path = file_path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, serde_yaml::to_string(self)?)?;
        Ok(path)
    }

    pub fn get(&self, name: &str) -> Result<&Host, AnyError> {
        self.hosts.get(name).ok_or_else(|| {
            let known = self.hosts.keys().cloned().collect::<Vec<_>>().join(", ");
            format!(
                "Unknown host '{}'. Known hosts: {}. Add one with 'rooz remote add <name> <ssh-url>'",
                name,
                if known.is_empty() { "(none)" } else { &known }
            )
            .into()
        })
    }

    pub fn select(&self, names: &[String]) -> Result<Vec<(String, Host)>, AnyError> {
        if names.is_empty() {
            if self.hosts.is_empty() {
                return Err(format!(
                    "No hosts configured. Add one with 'rooz remote add <name> <ssh-url>' (file: {})",
                    file_path()?.display()
                )
                .into());
            }
            return Ok(self
                .hosts
                .iter()
                .map(|(n, h)| (n.clone(), h.clone()))
                .collect());
        }
        names
            .iter()
            .map(|n| self.get(n).map(|h| (n.clone(), h.clone())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts() {
        let r = Remotes::parse(
            "hosts:\n  box1:\n    ssh: ssh://me@box1\n  box2:\n    ssh: ssh://me@box2\n    socket: /run/podman.sock\n",
        )
        .unwrap();
        assert_eq!(r.hosts.len(), 2);
        assert_eq!(r.hosts["box1"].socket, None);
        assert_eq!(r.hosts["box2"].socket.as_deref(), Some("/run/podman.sock"));
    }

    #[test]
    fn empty_file_is_no_hosts() {
        assert!(Remotes::parse("{}").unwrap().hosts.is_empty());
    }

    #[test]
    fn rejects_unknown_fields_and_bad_names() {
        assert!(Remotes::parse("hosts:\n  a:\n    ssh: x\n    port: 1\n").is_err());
        assert!(Remotes::parse("hosts:\n  Bad Name:\n    ssh: x\n").is_err());
        assert!(Remotes::parse("hosts:\n  add:\n    ssh: x\n").is_err());
    }

    #[test]
    fn name_rules() {
        assert!(validate_name("box-1_a").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("-x").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("rm").is_err());
        assert!(validate_name(&"a".repeat(33)).is_err());
    }

    #[test]
    fn roundtrips_without_empty_socket() {
        let mut r = Remotes::default();
        r.hosts.insert(
            "a".into(),
            Host {
                ssh: "ssh://a".into(),
                socket: None,
            },
        );
        let yaml = serde_yaml::to_string(&r).unwrap();
        assert!(!yaml.contains("socket"));
        assert_eq!(Remotes::parse(&yaml).unwrap(), r);
    }

    #[test]
    fn select_all_none_or_unknown() {
        let r = Remotes::parse("hosts:\n  a:\n    ssh: x\n  b:\n    ssh: y\n").unwrap();
        assert_eq!(r.select(&[]).unwrap().len(), 2);
        assert_eq!(r.select(&["b".into()]).unwrap()[0].0, "b");
        assert!(r.select(&["c".into()]).is_err());
    }
}
