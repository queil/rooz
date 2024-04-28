use colored::Colorize;

use crate::{
    api::{container, ExecApi, GitApi},
    id,
    labels::Labels,
    model::{
        config::{FileFormat, RoozCfg},
        types::{AnyError, ContainerResult, RunSpec},
        volume::RoozVolume,
    },
    ssh,
};

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
}

#[derive(Clone, Debug)]
pub struct RootRepoCloneResult {
    pub config: Option<RoozCfg>,
    pub dir: String,
}

fn get_clone_dir(root_dir: &str, git_ssh_url: &str) -> String {
    let clone_work_dir = git_ssh_url
        .split(&['/'])
        .last()
        .unwrap_or("repo")
        .replace(".git", "")
        .to_string();

    log::debug!("Clone dir: {}", &clone_work_dir);

    let work_dir = format!("{}/{}", root_dir, clone_work_dir.clone());

    log::debug!("Full clone dir: {:?}", &work_dir);
    work_dir
}

impl<'a> ExecApi<'a> {
    pub async fn read_rooz_config(
        &self,
        container_id: &str,
        clone_dir: &str,
        file_format: FileFormat,
        exact_path: Option<&str>,
    ) -> Result<Option<RoozCfg>, AnyError> {
        let file_path = match exact_path {
            Some(p) => format!("{}/{}", clone_dir, p.to_string()),
            None => format!("{}/.rooz.{}", clone_dir, file_format.to_string()),
        };

        let config = self
            .output(
                "rooz-cfg",
                &container_id,
                None,
                Some(vec![
                    "sh",
                    "-c",
                    format!(
                        "ls {} > /dev/null 2>&1 && cat `ls {} | head -1`",
                        file_path, file_path
                    )
                    .as_ref(),
                ]),
            )
            .await?;

        if config.is_empty() {
            Ok(None)
        } else {
            match RoozCfg::from_string(config, file_format) {
                Ok(cfg) => Ok(Some(cfg)),
                Err(e) => {
                    eprintln!(
                        "{}\n{}",
                        format!(
                            "WARNING: Could not read repo config ({})",
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
}

impl<'a> GitApi<'a> {
    async fn clone_from_spec(&self, spec: &CloneEnv, urls: &CloneUrls) -> Result<String, AnyError> {
        let mut clone_script = "export GIT_SSH_COMMAND='ssh -i /tmp/.ssh/id_ed25519 -o UserKnownHostsFile=/tmp/.ssh/known_hosts'\n".to_string();
        let all_urls: Vec<String> = match &urls {
            CloneUrls::Root { url } => vec![url.to_string()],
            CloneUrls::Extra { urls } => {
                urls.iter().map(|x| x.to_string()).collect::<Vec<String>>()
            }
        };
        for url in all_urls {
            let clone_dir = get_clone_dir(&spec.working_dir, &url);
            clone_script.push_str(
                format!(
                    "ls '{}/.git' > /dev/null 2>&1 || git clone --filter=blob:none {}\n",
                    &clone_dir, &url
                )
                .as_str(),
            )
        }

        let clone_cmd = container::inject(&clone_script, "clone.sh");

        let labels = Labels::new(Some(&spec.workspace_key), Some("git"));

        let vol = RoozVolume::work(&spec.workspace_key, &spec.working_dir);

        self.api
            .volume
            .ensure_mounts(&vec![vol.clone().into()], None)
            .await?;

        let run_spec = RunSpec {
            reason: "git-clone",
            image: &spec.image,
            uid: &spec.uid,
            work_dir: Some(&spec.working_dir),
            container_name: &id::random_suffix("rooz-git"),
            workspace_key: &spec.workspace_key,
            mounts: Some(vec![vol.to_mount(None), ssh::mount("/tmp/.ssh")]),
            entrypoint: Some(vec!["cat"]),
            privileged: false,
            force_recreate: false,
            auto_remove: true,
            labels,
            ..Default::default()
        };

        if let ContainerResult::Created { id } = self.api.container.create(run_spec).await? {
            self.api.container.start(&id).await?;
            self.api.exec.ensure_user(&id).await?;
            self.api
                .exec
                .chown(&id, &spec.uid, &spec.working_dir)
                .await?;

            self.api
                .exec
                .tty(
                    "git-clone",
                    &id,
                    true,
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

    async fn try_read_config(
        &self,
        container_id: &str,
        clone_dir: &str,
    ) -> Result<Option<RoozCfg>, AnyError> {
        let exec = self.api.exec;

        let rooz_cfg = if let Some(cfg) = exec
            .read_rooz_config(&container_id, &clone_dir, FileFormat::Toml, None)
            .await?
        {
            log::debug!("Config file found (TOML)");
            Some(cfg)
        } else if let Some(cfg) = exec
            .read_rooz_config(&container_id, &clone_dir, FileFormat::Yaml, None)
            .await?
        {
            log::debug!("Config file found (YAML)");
            Some(cfg)
        } else {
            log::debug!("No valid config file found");
            None
        };
        Ok(rooz_cfg)
    }

    pub async fn clone_root_repo(
        &self,
        url: &str,
        spec: &CloneEnv,
    ) -> Result<RootRepoCloneResult, AnyError> {
        let container_id = self
            .clone_from_spec(&spec, &CloneUrls::Root { url: url.into() })
            .await?;
        let clone_dir = get_clone_dir(&spec.working_dir, &url);
        let rooz_cfg = self.try_read_config(&container_id, &clone_dir).await?;
        self.api.container.kill(&container_id).await?;
        Ok(RootRepoCloneResult {
            config: rooz_cfg,
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
        self.api.container.kill(&container_id).await?;
        Ok(())
    }

    pub async fn clone_config_repo(
        &self,
        spec: CloneEnv,
        url: &str,
        path: &str,
    ) -> Result<Option<RoozCfg>, AnyError> {
        let container_id = self
            .clone_from_spec(
                &spec,
                &CloneUrls::Extra {
                    urls: vec![url.into()],
                },
            )
            .await?;
        let clone_dir = get_clone_dir(&spec.working_dir, &url);
        let file_format = FileFormat::from_path(path);
        let rooz_cfg = self
            .api
            .exec
            .read_rooz_config(&container_id, &clone_dir, file_format, Some(path))
            .await?;
        self.api.container.kill(&container_id).await?;
        Ok(rooz_cfg)
    }
}
