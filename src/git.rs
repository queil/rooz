use colored::Colorize;

use crate::{
    api::{container, ExecApi, GitApi},
    id,
    labels::Labels,
    model::{
        config::{FileFormat, RoozCfg},
        types::{AnyError, ContainerResult, GitCloneSpec, RunSpec},
        volume::RoozVolume,
    },
    ssh,
};

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
    ) -> Result<Option<RoozCfg>, AnyError> {
        let file_path = &format!("{}/.rooz.{}", clone_dir, file_format.to_string());

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
    pub async fn clone_repo(
        &self,
        image: &str,
        uid: &str,
        url: &str,
        workspace_key: &str,
        working_dir: &str,
    ) -> Result<(Option<RoozCfg>, GitCloneSpec), AnyError> {
        let clone_dir = get_clone_dir(working_dir, url);

        let clone_cmd = container::inject(
            format!(
                    r#"export GIT_SSH_COMMAND="ssh -i /tmp/.ssh/id_ed25519 -o UserKnownHostsFile=/tmp/.ssh/known_hosts"
                    ls "{}/.git" > /dev/null 2>&1 || git clone {}"#,
                &clone_dir, &url
            )
            .as_ref(),
            "clone.sh",
        );

        let labels = Labels::new(Some(&workspace_key), Some("git"));

        let vol = RoozVolume::work(workspace_key, working_dir);

        self.api
            .volume
            .ensure_mounts(&vec![vol.clone().into()], None)
            .await?;

        let run_spec = RunSpec {
            reason: "git-clone",
            image,
            uid: &uid,
            work_dir: Some(&working_dir),
            container_name: &id::random_suffix("rooz-git"),
            workspace_key,
            mounts: Some(vec![vol.to_mount(None), ssh::mount("/tmp/.ssh")]),
            entrypoint: Some(vec!["cat"]),
            privileged: false,
            force_recreate: false,
            auto_remove: true,
            labels: (&labels).into(),
            ..Default::default()
        };

        let container_result = self.api.container.create(run_spec).await?;
        self.api.container.start(container_result.id()).await?;
        let container_id = container_result.id();

        if let ContainerResult::Created { .. } = container_result {
            self.api.exec.ensure_user(container_id).await?;
            self.api
                .exec
                .chown(&container_id, uid, &working_dir)
                .await?;

            self.api
                .exec
                .tty(
                    "git-clone",
                    &container_id,
                    true,
                    None,
                    None,
                    Some(clone_cmd.iter().map(String::as_str).collect()),
                )
                .await?;
        };

        let exec = &self.api.exec;

        let rooz_cfg = if let Some(cfg) = exec
            .read_rooz_config(container_id, &clone_dir, FileFormat::Toml)
            .await?
        {
            log::debug!("Config file found (TOML)");
            Some(cfg)
        } else if let Some(cfg) = exec
            .read_rooz_config(container_id, &clone_dir, FileFormat::Yaml)
            .await?
        {
            log::debug!("Config file found (YAML)");
            Some(cfg)
        } else {
            log::debug!("No valid config file found");
            None
        };

        self.api.container.remove(&container_id, true).await?;

        Ok((rooz_cfg, GitCloneSpec { dir: clone_dir }))
    }
}
