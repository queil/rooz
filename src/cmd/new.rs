use crate::{
    age_utils,
    api::WorkspaceApi,
    cli::{WorkParams, WorkspacePersistence},
    constants,
    git::{CloneEnv, RootRepoCloneResult},
    labels::{self, Labels},
    model::{
        config::{FinalCfg, RoozCfg},
        types::{AnyError, EnterSpec, WorkSpec},
    },
};

impl<'a> WorkspaceApi<'a> {
    async fn new_core(
        &self,
        cfg_builder: &mut RoozCfg,
        cli_config: Option<RoozCfg>,
        cli_params: &WorkParams,
        work_spec: &WorkSpec<'a>,
        clone_spec: &CloneEnv,
        root_git_repo: Option<RootRepoCloneResult>,
        workspace_key: &str,
        force: bool,
        work_dir: &str,
        labels: &Labels,
    ) -> Result<EnterSpec, AnyError> {
        if let Some(c) = &cli_config {
            cfg_builder.from_config(c);
        }
        cfg_builder.from_cli(cli_params, None);

        log::debug!("Checking if vars need decryption");
        if let Some(vars) = age_utils::needs_decryption(cfg_builder.clone().vars) {
            log::debug!("Decrypting vars");
            let identity = self.read_age_identity().await?;
            let decrypted_kv = age_utils::decrypt(&identity, vars)?;
            cfg_builder.vars = Some(decrypted_kv);
        } else {
            log::debug!("No encrypted vars found");
        }

        cfg_builder.expand_vars()?;

        let cfg = FinalCfg::from(&*cfg_builder);

        self.api
            .image
            .ensure(&cfg.image, cli_params.pull_image)
            .await?;

        let network = self
            .ensure_sidecars(
                &cfg.sidecars,
                labels,
                workspace_key,
                force,
                cli_params.pull_image,
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
            container_working_dir: &root_git_repo
                .clone()
                .map(|r| r.dir)
                .unwrap_or(constants::WORK_DIR.to_string()),
            network: network.as_deref(),
            labels: (&work_labels).into(),
            privileged: cfg.privileged,
            ..*work_spec
        };

        let ws = self.create(&work_spec).await?;
        if !cfg.extra_repos.is_empty() {
            self.git
                .clone_extra_repos(clone_spec.clone(), cfg.extra_repos)
                .await?;
        }
        Ok(EnterSpec {
            workspace: ws,
            git_spec: root_git_repo,
            config: cfg_builder.clone(),
        })
    }

    pub async fn new(
        &self,
        cli_params: &WorkParams,
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
            .ensure(constants::DEFAULT_IMAGE, cli_params.pull_image)
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

        let clone_env = CloneEnv {
            image: constants::DEFAULT_IMAGE.into(),
            uid: orig_uid.to_string(),
            workspace_key: workspace_key.to_string(),
            working_dir: work_dir.to_string(),
        };

        match &RoozCfg::git_ssh_url(cli_params, &cli_config) {
            None => {
                let mut cfg_builder = RoozCfg::default().from_cli_env(cli_params.clone());
                self.new_core(
                    &mut cfg_builder,
                    cli_config,
                    cli_params,
                    &work_spec,
                    &clone_env,
                    None,
                    &workspace_key,
                    force,
                    work_dir,
                    &labels,
                )
                .await
            }

            Some(url) => match self.git.clone_root_repo(&url, &clone_env).await? {
                root_repo_result => {
                    let mut cfg_builder = RoozCfg::default().from_cli_env(cli_params.clone());

                    match &root_repo_result.config {
                        Some(c) => {
                            log::debug!("Config file applied.");
                            cfg_builder.from_config(c);
                        }
                        None => {
                            log::debug!("No valid config file found in the repository.");
                        }
                    }
                    self.new_core(
                        &mut cfg_builder,
                        cli_config,
                        cli_params,
                        &work_spec,
                        &clone_env,
                        Some(root_repo_result),
                        &workspace_key,
                        force,
                        work_dir,
                        &labels,
                    )
                    .await
                }
            },
        }
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
