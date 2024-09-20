use std::fs;

use crate::{
    api::WorkspaceApi,
    cli::{WorkParams, WorkspacePersistence},
    constants,
    git::{CloneEnv, RootRepoCloneResult},
    labels::{self, Labels},
    model::{
        config::{ConfigPath, ConfigSource, FileFormat, FinalCfg, RoozCfg},
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
    ) -> Result<EnterSpec, AnyError> {
        if let Some(c) = &cli_config {
            cfg_builder.from_config(c);
        }
        cfg_builder.from_cli(cli_params, None);
        cfg_builder.secrets = self.decrypt(cfg_builder.clone().secrets).await?;
        cfg_builder.expand_vars()?;

        let cfg = FinalCfg::from(&*cfg_builder);

        self.api
            .image
            .ensure(&cfg.image, cli_params.pull_image)
            .await?;

        let network = self
            .ensure_sidecars(
                &cfg.sidecars,
                workspace_key,
                force,
                cli_params.pull_image,
                &work_dir,
            )
            .await?;

        let labels = work_spec
            .labels
            .clone()
            .with_container(Some(constants::DEFAULT_CONTAINER_NAME))
            .with_runtime_config(cfg.clone());

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
            labels,
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
        cli_config_path: Option<ConfigSource>,
        persistence: Option<WorkspacePersistence>,
    ) -> Result<EnterSpec, AnyError> {
        let ephemeral = persistence.is_none();
        let orig_uid = constants::DEFAULT_UID.to_string();

        let (workspace_key, force, apply) = match persistence {
            Some(p) => (p.name.to_string(), p.replace, p.apply),
            None => (crate::id::random_suffix("tmp"), false, false),
        };

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

        let clone_env = CloneEnv {
            image: constants::DEFAULT_IMAGE.into(),
            uid: orig_uid.to_string(),
            workspace_key: workspace_key.to_string(),
            working_dir: work_dir.to_string(),
        };

        let mut labels = Labels {
            workspace: Labels::workspace(&workspace_key),
            role: Labels::role(labels::ROLE_WORK),
            ..Default::default()
        };

        let cli_cfg = if let Some(source) = &cli_config_path {
            match source {
                ConfigSource::Body {
                    value,
                    origin,
                    format,
                } => {
                    labels = Labels {
                        config_source: Labels::config_origin(&origin),
                        config_body: Labels::config_body(&value.to_string(format.clone())?),
                        ..labels
                    };
                    Some(value.clone())
                }
                ConfigSource::Path { value: path } => match path {
                    ConfigPath::File { path } => {
                        let body = fs::read_to_string(&path)?;
                        labels = Labels {
                            config_source: Labels::config_origin(&path),
                            config_body: Labels::config_body(&body),
                            ..labels
                        };
                        RoozCfg::deserialize_config(&body, FileFormat::from_path(&path))?
                    }
                    ConfigPath::Git { url, file_path } => {
                        let body = self
                            .git
                            .clone_config_repo(clone_env.clone(), &url, &file_path)
                            .await?;

                        labels = Labels {
                            config_source: Labels::config_origin(&path.to_string()),
                            ..labels
                        };

                        if let Some(b) = &body {
                            labels = Labels {
                                config_body: Labels::config_body(&b),
                                ..labels
                            }
                        };

                        match body {
                            Some(body) => {
                                let fmt = FileFormat::from_path(&file_path);
                                RoozCfg::deserialize_config(&body, fmt)?
                            }
                            None => None,
                        }
                    }
                },
            }
        } else {
            None
        };

        let work_spec = WorkSpec {
            uid: &orig_uid,
            container_working_dir: &work_dir,
            container_name: &workspace_key,
            workspace_key: &workspace_key,
            ephemeral,
            force_recreate: force,
            ..Default::default()
        };

        let enter_spec = match &RoozCfg::git_ssh_url(cli_params, &cli_cfg) {
            None => {
                let mut cfg_builder = RoozCfg::default().from_cli_env(cli_params.clone());
                self.new_core(
                    &mut cfg_builder,
                    cli_cfg,
                    cli_params,
                    &WorkSpec {
                        labels,
                        ..work_spec
                    },
                    &clone_env,
                    None,
                    &workspace_key,
                    force,
                    work_dir,
                )
                .await
            }

            Some(url) => match self.git.clone_root_repo(&url, &clone_env).await? {
                root_repo_result => {
                    let mut cfg_builder = RoozCfg::default().from_cli_env(cli_params.clone());
                    match &root_repo_result.config {
                        Some((body, format)) => match RoozCfg::deserialize_config(body, *format)? {
                            Some(c) => {
                                cfg_builder.from_config(&c);
                                log::debug!("Config file applied.");
                                let source = format!("{}//.rooz.{}", url, format.to_string());
                                labels = Labels {
                                    config_source: Labels::config_origin(&source),
                                    config_body: Labels::config_body(&body),
                                    ..labels
                                };
                            }
                            None => {
                                log::debug!("No valid config file found in the repository.");
                            }
                        },
                        None => {
                            log::debug!("No valid config file found in the repository.");
                        }
                    }

                    self.new_core(
                        &mut cfg_builder,
                        cli_cfg,
                        cli_params,
                        &WorkSpec {
                            labels,
                            ..work_spec
                        },
                        &clone_env,
                        Some(root_repo_result),
                        &workspace_key,
                        force,
                        work_dir,
                    )
                    .await
                }
            },
        };
        if let Some(true) = cli_params.start {
            self.start_workspace(&workspace_key).await?;
        }
        enter_spec
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
            shell: Some(vec![shell.into()]),
            ..config
        });

        self.enter(
            &workspace.workspace_key,
            working_dir.as_deref(),
            Some(cfg.shell.iter().map(|v| v.as_str()).collect::<Vec<_>>()),
            None,
            workspace.volumes,
            &workspace.orig_uid,
            root,
            true,
        )
        .await
    }
}
