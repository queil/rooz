use crate::{
    backend::WorkspaceApi,
    cli::{WorkParams, WorkspacePersistence},
    constants,
    labels::{self, Labels},
    types::{AnyError, RoozCfg, WorkSpec},
};

impl<'a> WorkspaceApi<'a> {
    pub async fn new(
        &self,
        spec: &WorkParams,
        cli_config: Option<RoozCfg>,
        persistence: Option<WorkspacePersistence>,
        root: bool,
    ) -> Result<String, AnyError> {
        let ephemeral = persistence.is_none();
        let orig_uid = constants::DEFAULT_UID.to_string();

        let (workspace_key, force, enter) = match persistence {
            Some(p) => (p.name.to_string(), p.force, p.enter),
            None => (crate::id::random_suffix("tmp"), false, true),
        };

        let labels = Labels::new(Some(&workspace_key), Some(labels::ROLE_WORK));
        if force {
            self.remove(&workspace_key, true).await?;
        }

        self.api
            .image
            .ensure(constants::DEFAULT_IMAGE, spec.pull_image)
            .await?;

        let orig_user = &RoozCfg::user(spec, &cli_config, &None);
        let home_dir = format!("/home/{}", &orig_user);
        let work_dir = format!("{}/work", &home_dir);

        let work_spec = WorkSpec {
            uid: &orig_uid,
            container_working_dir: &work_dir,
            container_name: &workspace_key,
            workspace_key: &workspace_key,
            labels: (&labels).into(),
            ephemeral,
            privileged: spec.privileged,
            force_recreate: force,
            user: orig_user,
            ..Default::default()
        };

        match &spec.git_ssh_url {
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
                    )
                    .await?;
                let work_labels = labels
                    .clone()
                    .with_container(Some(constants::DEFAULT_CONTAINER_NAME));
                let work_spec = WorkSpec {
                    image,
                    shell: &RoozCfg::shell(spec, &cli_config, &None),
                    caches: Some(RoozCfg::caches(spec, &cli_config, &None)),
                    network: network.as_deref(),
                    labels: (&work_labels).into(),
                    ..work_spec
                };

                let ws = self.create(&work_spec).await?;
                let volumes = ws.volumes;
                if enter {
                    self.enter(
                        &workspace_key,
                        Some(&work_spec.container_working_dir),
                        Some(&home_dir),
                        &work_spec.shell.as_ref(),
                        None,
                        volumes,
                        &orig_uid,
                        root,
                        ephemeral,
                    )
                    .await?;
                }
                return Ok(ws.container_id);
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
                        log::debug!("Config read from .rooz.toml in the cloned repo");

                        let image = &RoozCfg::image(spec, &cli_config, &repo_config);
                        self.api.image.ensure(&image, spec.pull_image).await?;
                        let network = self
                            .ensure_sidecars(
                                &RoozCfg::sidecars(&cli_config, &repo_config),
                                &labels,
                                &workspace_key,
                                force,
                                spec.pull_image,
                            )
                            .await?;
                        let work_spec = WorkSpec {
                            image,
                            shell: &RoozCfg::shell(spec, &cli_config, &repo_config),
                            caches: Some(RoozCfg::caches(spec, &cli_config, &repo_config)),
                            container_working_dir: &git_spec.dir,
                            git_vol_mount: Some(git_spec.mount),
                            network: network.as_deref(),
                            ..work_spec
                        };

                        let ws = self.create(&work_spec).await?;
                        let mut volumes = ws.volumes;
                        volumes.push(git_spec.volume);
                        if enter {
                            self.enter(
                                &workspace_key,
                                Some(&git_spec.dir),
                                Some(&home_dir),
                                &work_spec.shell,
                                None,
                                volumes,
                                &orig_uid,
                                root,
                                ephemeral,
                            )
                            .await?;
                        }
                        return Ok(ws.container_id);
                    }
                }
            }
        };
    }
}
