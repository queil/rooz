use crate::{
    api::{container, GitApi},
    id,
    labels::Labels,
    model::{
        config::RoozCfg,
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

impl<'a> GitApi<'a> {
    pub async fn clone_repo(
        &self,
        image: &str,
        uid: &str,
        url: &str,
        workspace_key: &str,
        working_dir: &str,
    ) -> Result<(Option<Result<RoozCfg, AnyError>>, GitCloneSpec), AnyError> {
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

        let rooz_cfg = self
            .api
            .exec
            .output(
                "rooz-toml",
                &container_id,
                None,
                Some(vec![
                    "sh",
                    "-c",
                    format!(
                        "ls {}/.rooz.toml > /dev/null 2>&1 && cat {}/.rooz.toml",
                        clone_dir, clone_dir
                    )
                    .as_ref(),
                ]),
            )
            .await?;

        log::debug!("Repo config result: {}", &rooz_cfg);

        self.api.container.remove(&container_id, true).await?;

        Ok((
            if rooz_cfg.is_empty() {
                None
            } else {
                Some(RoozCfg::from_string(rooz_cfg))
            },
            GitCloneSpec { dir: clone_dir },
        ))
    }
}
