use crate::util::git;
use std::path::{Path, PathBuf};

pub const TRUSTED_CONFIGS_ENV: &str = "ROOZ_TRUSTED_CONFIGS";

#[derive(Debug, Default)]
pub struct TrustedConfigs {
    repos: Vec<String>,
    dirs: Vec<PathBuf>,
}

impl TrustedConfigs {
    pub fn from_env() -> Self {
        Self::parse(
            std::env::var(TRUSTED_CONFIGS_ENV).ok().as_deref(),
            std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        )
    }

    pub fn parse(value: Option<&str>, home: Option<&Path>) -> Self {
        let mut trusted = Self::default();
        for entry in value
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            if entry.contains(':') {
                trusted.repos.push(entry.to_string());
                continue;
            }
            let dir = match (entry.strip_prefix("~/"), home) {
                (Some(rest), Some(home)) => home.join(rest),
                (Some(_), None) => continue,
                (None, _) => PathBuf::from(entry),
            };
            if let Ok(dir) = dir.canonicalize() {
                trusted.dirs.push(dir);
            }
        }
        trusted
    }

    pub fn trusts_repo(&self, url: &str) -> bool {
        if !self.repos.iter().any(|r| r == url) {
            return false;
        }
        if git::is_cleartext(url) {
            eprintln!(
                "WARN: '{}' is listed in {} but is fetched over a cleartext transport, so \
                 anyone on the network path can substitute it - not trusting it.",
                url, TRUSTED_CONFIGS_ENV
            );
            return false;
        }
        true
    }

    pub fn trusts_files<'p>(&self, paths: impl IntoIterator<Item = &'p str>) -> bool {
        paths.into_iter().all(|p| {
            Path::new(p)
                .canonicalize()
                .is_ok_and(|p| self.dirs.iter().any(|d| p.starts_with(d)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rooz-trust-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn file(dir: &Path, name: &str) -> String {
        let p = dir.join(name);
        fs::write(&p, "image: alpine\n").unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn nothing_is_trusted_by_default() {
        let t = TrustedConfigs::parse(None, None);
        assert!(!t.trusts_repo("git@github.com:me/configs"));
        let dir = scratch("default");
        assert!(!t.trusts_files([file(&dir, "a.yaml").as_str()]));
    }

    #[test]
    fn repos_match_exactly_as_listed() {
        let t = TrustedConfigs::parse(
            Some(" git@github.com:me/configs , https://github.com/me/other.git,"),
            None,
        );
        assert!(t.trusts_repo("git@github.com:me/configs"));
        assert!(t.trusts_repo("https://github.com/me/other.git"));
        assert!(!t.trusts_repo("git@github.com:me/configs.git"));
        assert!(!t.trusts_repo("https://github.com/me/other"));
        assert!(!t.trusts_repo("git@github.com:me/configs-evil"));
    }

    #[test]
    fn cleartext_repos_are_never_trusted() {
        let t = TrustedConfigs::parse(Some("http://h/me/configs,git://h/me/configs"), None);
        assert!(!t.trusts_repo("http://h/me/configs"));
        assert!(!t.trusts_repo("git://h/me/configs"));
    }

    #[test]
    fn files_must_all_resolve_under_a_trusted_dir() {
        let root = scratch("dirs");
        let mine = root.join("mine");
        let other = root.join("other");
        fs::create_dir_all(&mine).unwrap();
        fs::create_dir_all(&other).unwrap();
        let t = TrustedConfigs::parse(Some(mine.to_str().unwrap()), None);

        let cfg = file(&mine, "ws.yaml");
        let base = file(&mine, "base.yaml");
        let foreign = file(&other, "base.yaml");
        assert!(t.trusts_files([cfg.as_str(), base.as_str()]));
        assert!(!t.trusts_files([cfg.as_str(), foreign.as_str()]));
        assert!(!t.trusts_files([mine.join("missing.yaml").to_str().unwrap()]));
        let sibling = root.join("mine-not");
        fs::create_dir_all(&sibling).unwrap();
        assert!(!t.trusts_files([file(&sibling, "x.yaml").as_str()]));
    }

    #[test]
    fn a_symlink_out_of_a_trusted_dir_is_not_trusted() {
        let root = scratch("symlink");
        let mine = root.join("mine");
        fs::create_dir_all(&mine).unwrap();
        let outside = file(&root, "planted.yaml");
        let link = mine.join("base.yaml");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let t = TrustedConfigs::parse(Some(mine.to_str().unwrap()), None);
        assert!(!t.trusts_files([link.to_str().unwrap()]));
    }

    #[test]
    fn home_relative_dirs_expand() {
        let home = scratch("home");
        fs::create_dir_all(home.join("configs")).unwrap();
        let t = TrustedConfigs::parse(Some("~/configs"), Some(&home));
        assert!(t.trusts_files([file(&home.join("configs"), "a.yaml").as_str()]));
        assert!(
            TrustedConfigs::parse(Some("~/configs"), None)
                .dirs
                .is_empty()
        );
    }
}
