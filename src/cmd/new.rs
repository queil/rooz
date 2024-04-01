use crate::{
    api::WorkspaceApi,
    cli::{WorkParams, WorkspacePersistence},
    constants,
    labels::{self, Labels},
    model::{
        config::RoozCfg,
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

        let orig_user = &RoozCfg::user(spec, &cli_config, &None);
        let work_dir = constants::WORK_DIR;

        let work_spec = WorkSpec {
            uid: &orig_uid,
            container_working_dir: &work_dir,
            container_name: &workspace_key,
            workspace_key: &workspace_key,
            labels: (&labels).into(),
            ephemeral,
            force_recreate: force,
            user: orig_user,
            ..Default::default()
        };

        match &RoozCfg::git_ssh_url(spec, &cli_config) {
            None => {
                let image = &RoozCfg::image(spec, &cli_config, &None);
                self.api.image.ensure(&image, spec.pull_image).await?;

                let network = self
                    .ensure_sidecars(
                        &RoozCfg::sidecars(&cli_config, &None),
                        &labels,
                        &workspace_key,
                        force,
                        spec.pull_image,
                        &work_dir,
                    )
                    .await?;
                let work_labels = labels
                    .clone()
                    .with_container(Some(constants::DEFAULT_CONTAINER_NAME));
                let work_spec = WorkSpec {
                    image,
                    caches: Some(RoozCfg::caches(spec, &cli_config, &None)),
                    env_vars: RoozCfg::env_vars(&cli_config, &None),
                    ports: RoozCfg::ports(&cli_config, &None),
                    network: network.as_deref(),
                    labels: (&work_labels).into(),
                    privileged: RoozCfg::privileged(spec, &cli_config, &None),
                    ..work_spec
                };

                let ws = self.create(&work_spec).await?;
                return Ok(EnterSpec {
                    workspace: ws,
                    git_spec: None,
                    git_repo_config: None,
                });
            }
            Some(url) => {
                match self
                    .git
                    .clone_repo(
                        constants::DEFAULT_IMAGE,
                        &orig_uid,
                        url,
                        &workspace_key,
                        &work_dir,
                    )
                    .await?
                {
                    (repo_config, git_spec) => {
                        if let Some(_) = &repo_config {
                            log::debug!("Config read from .rooz.toml in the cloned repo");
                        } else {
                            log::debug!(".rooz.toml ignored")
                        }

                        let image = &RoozCfg::image(spec, &cli_config, &repo_config);
                        self.api.image.ensure(&image, spec.pull_image).await?;
                        let network = self
                            .ensure_sidecars(
                                &RoozCfg::sidecars(&cli_config, &repo_config),
                                &labels,
                                &workspace_key,
                                force,
                                spec.pull_image,
                                &work_dir,
                            )
                            .await?;
                        let work_labels = labels
                            .clone()
                            .with_container(Some(constants::DEFAULT_CONTAINER_NAME));

                        let work_spec = WorkSpec {
                            image,
                            caches: Some(RoozCfg::caches(spec, &cli_config, &repo_config)),
                            env_vars: RoozCfg::env_vars(&cli_config, &repo_config),
                            ports: RoozCfg::ports(&cli_config, &repo_config),
                            container_working_dir: &git_spec.dir,
                            network: network.as_deref(),
                            labels: (&work_labels).into(),
                            privileged: RoozCfg::privileged(spec, &cli_config, &repo_config),
                            ..work_spec
                        };

                        let ws = self.create(&work_spec).await?;

                        return Ok(EnterSpec {
                            workspace: ws,
                            git_spec: Some(git_spec),
                            git_repo_config: repo_config,
                        });
                    }
                }
            }
        };
    }

    pub async fn tmp(&self, spec: &WorkParams, root: bool, shell: &str) -> Result<(), AnyError> {
        let EnterSpec {
            workspace,
            git_spec,
            git_repo_config,
        } = self.new(spec, None, None).await?;

        let working_dir = git_spec
            .map(|v| (&v).dir.to_string())
            .or(Some(workspace.working_dir));

        self.enter(
            &workspace.workspace_key,
            working_dir.as_deref(),
            &RoozCfg::shell(shell, &None, &git_repo_config),
            None,
            workspace.volumes,
            &workspace.orig_uid,
            root,
            true,
        )
        .await
    }
}
