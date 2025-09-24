use std::fs;

use crate::{
    api::WorkspaceApi,
    cli::WorkParams,
    config::{
        config::{ConfigPath, ConfigSource, ConfigType, FileFormat, RoozCfg},
        runtime::RuntimeConfig,
    },
    constants,
    model::types::{AnyError, EnterSpec, WorkSpec},
    util::{
        git::{CloneEnv, RootRepoCloneResult},
        id,
        labels::{self, Labels},
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
        self.config
            .decrypt(cfg_builder, &self.api.system_config.age_identity()?)
            .await?;
        cfg_builder.expand_vars()?;

        let cfg = RuntimeConfig::from(&*cfg_builder);

        self.api
            .image
            .ensure(&cfg.image, cli_params.pull_image)
            .await?;

        if let Some(home_from_image) = &cfg.home_from_image {
            self.api
                .image
                .ensure(&home_from_image, cli_params.pull_image)
                .await?;
        }

        let network = self
            .ensure_sidecars(
                &cfg.sidecars,
                workspace_key,
                force,
                cli_params.pull_image,
                &work_dir,
            )
            .await?;

        let mut labels = work_spec.labels.clone();

        labels.extend(&[
            Labels::container(constants::DEFAULT_CONTAINER_NAME),
            Labels::config_runtime(&cfg.to_string().unwrap()),
        ]);

        self.config
            .store(
                workspace_key,
                &ConfigType::Runtime,
                &cfg.clone().to_string()?,
            )
            .await?;

        let work_spec = WorkSpec {
            image: &cfg.image,
            home_from_image: cfg.home_from_image.as_deref(),
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
            entrypoint: constants::default_entrypoint(),
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

    async fn get_cli_config(
        &self,
        workspace_key: &str,
        cli_config_path: &Option<ConfigSource>,
        clone_env: &CloneEnv,
    ) -> Result<(Option<RoozCfg>, Option<Labels>), AnyError> {
        let val = if let Some(source) = &cli_config_path {
            let (origin, body, rooz_cfg): (String, Option<String>, Option<RoozCfg>) = match source {
                ConfigSource::Update {
                    value,
                    origin,
                    format,
                } => {
                    let body = value.to_string(format.clone())?;
                    (origin.to_string(), Some(body), Some(value.clone()))
                }
                ConfigSource::Path { value: path } => match path {
                    ConfigPath::File { path } => {
                        let body = fs::read_to_string(&path)?;
                        let absolute_path =
                            std::path::absolute(path)?.to_string_lossy().into_owned();
                        (
                            absolute_path.to_string(),
                            Some(body.clone()),
                            RoozCfg::deserialize_config(&body, FileFormat::from_path(&path))?,
                        )
                    }
                    ConfigPath::Git { url, file_path } => {
                        let body = self
                            .git
                            .clone_config_repo(clone_env.clone(), &url, &file_path)
                            .await?;

                        let rooz_cfg = match body.clone() {
                            Some(body) => {
                                let fmt = FileFormat::from_path(&file_path);
                                RoozCfg::deserialize_config(&body, fmt)?
                            }
                            None => None,
                        };

                        (path.to_string(), body, rooz_cfg)
                    }
                },
            };
            let mut labels = Labels::default();
            labels.append(Labels::config_origin(&origin));

            self.config
                .store(workspace_key, &ConfigType::Origin, &origin)
                .await?;

            if let Some(b) = &body {
                labels.append(Labels::config_body(b));

                self.config
                    .store(workspace_key, &ConfigType::Body, &b)
                    .await?;
            }
            (rooz_cfg, Some(labels.clone()))
        } else {
            (None, None)
        };

        Ok(val)
    }

    pub async fn new(
        &self,
        workspace_key: &str,
        cli_params: &WorkParams,
        cli_config_path: Option<ConfigSource>,
        ephemeral: bool,
    ) -> Result<EnterSpec, AnyError> {
        let orig_uid = cli_params
            .uid
            .map(|x| x.to_string())
            .unwrap_or(constants::DEFAULT_UID.to_string());

        let mut labels = Labels::from(&[
            Labels::workspace(&workspace_key),
            Labels::role(labels::ROLE_WORK),
        ]);

        self.api
            .image
            .ensure(constants::DEFAULT_IMAGE, cli_params.pull_image)
            .await?;

        let work_dir = constants::WORK_DIR;

        let clone_env = CloneEnv {
            uid: orig_uid.to_string(),
            workspace_key: workspace_key.to_string(),
            working_dir: work_dir.to_string(),
            ..Default::default()
        };

        let (cli_cfg, cli_cfg_labels) = self
            .get_cli_config(workspace_key, &cli_config_path, &clone_env)
            .await?;

        if let Some(values) = cli_cfg_labels {
            labels.extend_with_labels(values);
        }

        let work_spec = WorkSpec {
            uid: &orig_uid,
            container_working_dir: &work_dir,
            container_name: &workspace_key,
            workspace_key: &workspace_key,
            ephemeral,
            force_recreate: false,
            ..Default::default()
        };
        let mut cfg_builder = RoozCfg::default().from_cli_env(cli_params.clone());
        let root_repo_result = match &RoozCfg::git_ssh_url(cli_params, &cli_cfg) {
            Some(url) => {
                let result = self.git.clone_root_repo(&url, &clone_env).await?;
                match (&result.config, &cli_config_path) {
                    (Some(_), Some(ConfigSource::Update { .. })) => {
                        log::debug!("Ignoring the in-repo config file in update mode");
                    }
                    (Some((body, format)), _) => {
                        match RoozCfg::deserialize_config(body, *format)? {
                            Some(c) => {
                                cfg_builder.from_config(&c);
                                log::debug!("Config file applied.");
                                let origin = format!("{}//.rooz.{}", url, format.to_string());
                                labels.append(Labels::config_origin(&origin));
                                labels.append(Labels::config_body(&body));
                                self.config
                                    .store(workspace_key, &ConfigType::Origin, &origin)
                                    .await?;

                                self.config
                                    .store(workspace_key, &ConfigType::Body, &body)
                                    .await?;
                            }
                            None => {
                                log::debug!("No valid config file found in the repository.");
                            }
                        }
                    }
                    (None, _) => {
                        log::debug!("No valid config file found in the repository.");
                    }
                }

                Some(result)
            }
            None => None,
        };

        let enter_spec = self
            .new_core(
                &mut cfg_builder,
                cli_cfg,
                cli_params,
                &WorkSpec {
                    labels,
                    ..work_spec
                },
                &clone_env,
                root_repo_result,
                &workspace_key,
                false,
                work_dir,
            )
            .await;

        if let Some(true) = cli_params.start {
            self.start(&workspace_key).await?;
        }
        enter_spec
    }

    pub async fn tmp(&self, spec: &WorkParams, root: bool, shell: &str) -> Result<(), AnyError> {
        let EnterSpec {
            workspace,
            git_spec,
            config,
        } = self
            .new(&id::random_suffix("tmp"), spec, None, true)
            .await?;

        let working_dir = git_spec
            .map(|v| (&v).dir.to_string())
            .or(Some(workspace.working_dir));

        let cfg = RuntimeConfig::from(&RoozCfg {
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
