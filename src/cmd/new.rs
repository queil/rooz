use crate::{
    api::WorkspaceApi,
    cli::{WorkParams, WorkspacePersistence},
    constants,
    git::{CloneSpec, RootRepoCloneResult},
    labels::{self, Labels},
    model::{
        config::{FinalCfg, RoozCfg},
        types::{AnyError, EnterSpec, WorkSpec},
    },
};

impl<'a> WorkspaceApi<'a> {
    pub async fn new(
        &self,
        spec: &WorkParams,
        cli_config: Option<RoozCfg>,
        persistence: Option<WorkspacePersistence>,
    ) -> Result<EnterSpec, AnyError> {
        let ephemeral = persistence.is_none();
        let orig_uid = constants::DEFAULT_UID.to_string();

        let (workspace_key, force, apply) = match persistence {
            Some(p) => (p.name.to_string(), p.replace, p.apply),
            None => (crate::id::random_suffix("tmp"), false, false),
        };

        let labels = Labels::new(Some(&workspace_key), Some(labels::ROLE_WORK));

        if apply {
            self.remove_containers_only(&workspace_key, true).await?;
        }

        if force {
            self.remove(&workspace_key, true).await?;
        }

        self.api
            .image
            .ensure(constants::DEFAULT_IMAGE, spec.pull_image)
            .await?;

        let work_dir = constants::WORK_DIR;

        let work_spec = WorkSpec {
            uid: &orig_uid,
            container_working_dir: &work_dir,
            container_name: &workspace_key,
            workspace_key: &workspace_key,
            labels: (&labels).into(),
            ephemeral,
            force_recreate: force,
            ..Default::default()
        };

        let clone_spec = CloneSpec {
            image: constants::DEFAULT_IMAGE.into(),
            uid: orig_uid.to_string(),
            workspace_key: workspace_key.to_string(),
            working_dir: work_dir.to_string(),
        };

        match &RoozCfg::git_ssh_url(spec, &cli_config) {
            None => {
                let mut cfg_builder = RoozCfg::default().from_cli_env(spec.clone());
                if let Some(c) = &cli_config {
                    cfg_builder = cfg_builder.from_config(c.clone());
                }
                cfg_builder = cfg_builder.from_cli(spec.clone(), None);
                let cfg = FinalCfg::from(&cfg_builder);

                self.api.image.ensure(&cfg.image, spec.pull_image).await?;

                let network = self
                    .ensure_sidecars(
                        &cfg.sidecars,
                        &labels,
                        &workspace_key,
                        force,
                        spec.pull_image,
                        &work_dir,
                    )
                    .await?;
                let work_labels = labels
                    .clone()
                    .with_container(Some(constants::DEFAULT_CONTAINER_NAME))
                    .with_config(cfg.clone());
                let work_spec = WorkSpec {
                    image: &cfg.image,
                    user: &cfg.user,
                    caches: Some(cfg.caches),
                    env_vars: Some(cfg.env),
                    ports: Some(cfg.ports),
                    network: network.as_deref(),
                    labels: (&work_labels).into(),
                    privileged: cfg.privileged,
                    ..work_spec
                };

                let ws = self.create(&work_spec).await?;
                if !cfg.extra_repos.is_empty() {
                    self.git
                        .clone_extra_repos(clone_spec, cfg.extra_repos)
                        .await?;
                }
                return Ok(EnterSpec {
                    workspace: ws,
                    git_spec: None,
                    config: cfg_builder,
                });
            }

            Some(url) => match self.git.clone_root_repo(&url, &clone_spec).await? {
                RootRepoCloneResult {
                    config: repo_config,
                    dir: clone_dir,
                } => {
                    let mut cfg_builder = RoozCfg::default().from_cli_env(spec.clone());

                    match &repo_config {
                        Some(c) => {
                            log::debug!("Config file applied.");
                            cfg_builder = cfg_builder.from_config(c.clone());
                        }
                        None => {
                            log::debug!("No valid config file found in the repository.");
                        }
                    }

                    if let Some(c) = &cli_config {
                        cfg_builder = cfg_builder.from_config(c.clone());
                    }
                    cfg_builder = cfg_builder.from_cli(spec.clone(), None);
                    let cfg = FinalCfg::from(&cfg_builder);

                    self.api.image.ensure(&cfg.image, spec.pull_image).await?;
                    let network = self
                        .ensure_sidecars(
                            &cfg.sidecars,
                            &labels,
                            &workspace_key,
                            force,
                            spec.pull_image,
                            &work_dir,
                        )
                        .await?;
                    let work_labels = labels
                        .clone()
                        .with_container(Some(constants::DEFAULT_CONTAINER_NAME))
                        .with_config(cfg.clone());

                    let work_spec = WorkSpec {
                        image: &cfg.image,
                        user: &cfg.user,
                        caches: Some(cfg.caches),
                        env_vars: Some(cfg.env),
                        ports: Some(cfg.ports),
                        container_working_dir: &clone_dir,
                        network: network.as_deref(),
                        labels: (&work_labels).into(),
                        privileged: cfg.privileged,
                        ..work_spec
                    };

                    let ws = self.create(&work_spec).await?;
                    if !cfg.extra_repos.is_empty() {
                        self.git
                            .clone_extra_repos(clone_spec, cfg.extra_repos)
                            .await?;
                    }
                    return Ok(EnterSpec {
                        workspace: ws,
                        git_spec: Some(RootRepoCloneResult {
                            config: repo_config.clone(),
                            dir: clone_dir,
                        }),
                        config: cfg_builder,
                    });
                }
            },
        };
    }

    pub async fn tmp(&self, spec: &WorkParams, root: bool, shell: &str) -> Result<(), AnyError> {
        let EnterSpec {
            workspace,
            git_spec,
            config,
        } = self.new(spec, None, None).await?;

        let working_dir = git_spec
            .map(|v| (&v).dir.to_string())
            .or(Some(workspace.working_dir));

        let cfg = FinalCfg::from(&RoozCfg {
            shell: Some(shell.into()),
            ..config
        });

        self.enter(
            &workspace.workspace_key,
            working_dir.as_deref(),
            Some(&cfg.shell),
            None,
            workspace.volumes,
            &workspace.orig_uid,
            root,
            true,
        )
        .await
    }
}
