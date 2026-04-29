use crate::api::VolumeApi;
use crate::api::config::{ConfigBody, LocalReader};
use crate::model::types::VolumeResult;
use crate::{
    api::WorkspaceApi,
    cli::WorkParams,
    config::{
        config::{ConfigPath, ConfigSource, FileFormat, RoozCfg},
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
use bollard::errors::Error;
use bollard_stubs::models::NetworkCreateRequest;
use std::collections::HashMap;
use std::fs;

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
    ) -> Result<EnterSpec, AnyError> {
        if let Some(c) = &cli_config {
            cfg_builder.from_config(c);
        }
        cfg_builder.from_cli(cli_params, None);
        self.config
            .decrypt(
                cfg_builder,
                &self.api.get_system_config().await?.age_identity()?,
            )
            .await?;
        cfg_builder.expand_vars()?;

        let cfg = RuntimeConfig::from(&*cfg_builder);

        self.api
            .image
            .ensure(&cfg.image, cli_params.pull_image)
            .await?;

        let volumes_v2 =
            VolumeApi::create_volume_specs(workspace_key, &cfg.data, &cfg.all_mounts(), true);

        let mounts_all = &cfg
            .mounts
            .iter()
            .map(|(target, source)| (target.to_string(), source.resolve_key(target)))
            .collect::<HashMap<String, String>>();

        let volume_results = self.api.volume.ensure_volumes_v2(&volumes_v2).await?;

        let home_dir = format!("/home/{}", &cfg.user);
        let mounts_config = self
            .api
            .volume
            .mounts_with_sources(&volumes_v2, mounts_all, true);

        let real_mounts = VolumeApi::real_mounts_v2(mounts_config.clone(), Some(&home_dir));

        let cfg = RuntimeConfig {
            real_mounts: real_mounts.clone(),
            ..cfg.clone()
        };

        let mut cfg2 = cfg.clone();

        let mounts_v2 = self.api.volume.mounts_v2(&real_mounts).await?;
        for (t, m) in real_mounts.clone() {
            //TODO: when initializing volumes both here in sidecars we should verify
            // if each file exists and if not create them
            if let VolumeResult::Created {} = volume_results[&m.volume_name] {
                self.api
                    .volume
                    .populate_volume(t, m, Some(work_spec.uid.to_string().parse::<i32>()?))
                    .await?;
            }
        }

        let mut labels = work_spec.labels.clone();

        let internal_network = &constants::internal_network(workspace_key);
        let egress_network = &constants::egress_network(workspace_key);

        match self
            .api
            .client
            .create_network(NetworkCreateRequest {
                name: egress_network.to_string(),
                labels: Some(labels.clone().into()),
                ..Default::default()
            })
            .await
        {
            Ok(_) => {}
            Err(Error::DockerResponseServerError {
                status_code: 409, ..
            }) => {
                log::debug!("Network already exists: {}. Skipping", egress_network);
            }
            Err(e) => return Err(e.into()),
        };

        if !cfg2.sidecars.is_empty() {
            match self
                .api
                .client
                .create_network(NetworkCreateRequest {
                    name: internal_network.to_string(),
                    internal: Some(true),
                    labels: Some(labels.clone().into()),
                    ..Default::default()
                })
                .await
            {
                Ok(_) => {}
                Err(Error::DockerResponseServerError {
                    status_code: 409, ..
                }) => {
                    log::debug!("Network already exists: {}. Skipping", internal_network);
                }
                Err(e) => return Err(e.into()),
            };
        }

        let cfg2 = self
            .ensure_sidecars(&mut cfg2, workspace_key, force, cli_params.pull_image)
            .await?;

        labels.extend(&[Labels::container(constants::DEFAULT_CONTAINER_NAME)]);

        self.config
            .store_runtime(workspace_key, &cfg2.clone().to_string()?)
            .await?;

        let work_spec = WorkSpec {
            image: &cfg2.image,
            user: &cfg2.user,
            caches: Some(cfg2.caches),
            env_vars: Some(cfg2.env),
            ports: Some(cfg2.ports),
            container_working_dir: &root_git_repo
                .clone()
                .map(|r| r.dir)
                .unwrap_or(constants::WORK_DIR.to_string()),
            default_network: Some(egress_network.as_str()),
            additional_networks: if !cfg2.sidecars.is_empty() {
                Some(vec![internal_network.as_str()])
            } else {
                None
            },
            labels,
            privileged: cfg2.privileged,
            init: cfg2.init,
            args: (if cfg2.args.len() > 0 {
                Some(&cfg2.args)
            } else {
                None
            })
            .as_ref()
            .map(|x| x.iter().map(|z| z.as_ref()).collect()),
            command: (if cfg2.command.len() > 0 {
                Some(&cfg2.command)
            } else {
                None
            })
            .as_ref()
            .map(|x| x.iter().map(|z| z.as_ref()).collect()),
            mounts: mounts_v2,
            install: cfg2.install,
            ..*work_spec
        };

        let ws = self.create(&work_spec, &real_mounts).await?;
        if !cfg2.extra_repos.is_empty() {
            self.git
                .clone_extra_repos(clone_spec.clone(), cfg2.extra_repos)
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
    ) -> Result<Option<RoozCfg>, AnyError> {
        let val = if let Some(source) = &cli_config_path {
            let (origin, body, extends_body, rooz_cfg): (
                String,
                Option<String>,
                Option<String>,
                Option<RoozCfg>,
            ) = match source {
                ConfigSource::Update {
                    value,
                    origin,
                    format,
                } => {
                    let body = value.to_string(format.clone())?;
                    let (cfg, base_body) = if value.bases.is_some() {
                        if let Ok(ConfigPath::File { path }) = ConfigPath::from_str(origin) {
                            let reader = LocalReader {};
                            let (merged, individual_bases) = self
                                .config
                                .resolve_extends_chain(&reader, &path, value.clone(), 0)
                                .await?;
                            let bases_yaml = individual_bases
                                .iter()
                                .map(|(p, b)| b.to_string(*format).map(|yaml| format!("# {}\n{}", p, yaml)))
                                .collect::<Result<Vec<_>, _>>()?
                                .join("\n---\n");
                            (Some(merged), if bases_yaml.is_empty() { None } else { Some(bases_yaml) })
                        } else {
                            (Some(value.clone()), None)
                        }
                    } else {
                        (Some(value.clone()), None)
                    };
                    (origin.to_string(), Some(body), base_body, cfg)
                }
                ConfigSource::Path { value: path } => match path {
                    ConfigPath::File { path } => {
                        let body = fs::read_to_string(&path)?;
                        let absolute_path =
                            std::path::absolute(path)?.to_string_lossy().into_owned();
                        let fmt = FileFormat::from_path(&path);
                        let cfg = RoozCfg::deserialize_config(&body, fmt)?;

                        let (cfg, base_body) = match cfg {
                            Some(c) if c.bases.is_some() => {
                                let reader = LocalReader {};
                                let (merged, individual_bases) = self
                                    .config
                                    .resolve_extends_chain(&reader, path.as_str(), c, 0)
                                    .await?;
                                let bases_yaml = individual_bases
                                    .iter()
                                    .map(|(path, b)| b.to_string(fmt).map(|yaml| format!("# {}\n{}", path, yaml)))
                                    .collect::<Result<Vec<_>, _>>()?
                                    .join("\n---\n");
                                let base_body = if bases_yaml.is_empty() { None } else { Some(bases_yaml) };
                                (Some(merged), base_body)
                            }
                            other => (other, None),
                        };
                        (absolute_path, Some(body.clone()), base_body, cfg)
                    }
                    ConfigPath::Git { url, file_path } => {
                        let (result, _) = self
                            .git
                            .clone_config_repo(clone_env.clone(), &url, &file_path)
                            .await?;

                        let (rooz_cfg, main_body, base_body) = match result {
                            Some(ConfigBody { body, bases, merged }) => {
                                let fmt = FileFormat::from_path(&file_path);
                                let cfg = merged
                                    .map(Ok)
                                    .unwrap_or_else(|| RoozCfg::deserialize_config(&body, fmt).map(|o| o.unwrap()))?;
                                (Some(cfg), Some(body), bases)
                            }
                            None => (None, None, None),
                        };

                        (path.to_string(), main_body, base_body, rooz_cfg)
                    }
                },
            };

            self.config
                .store(workspace_key, &origin, &body.unwrap())
                .await?;
            self.config
                .store_bases(workspace_key, extends_body.as_deref().unwrap_or(""))
                .await?;

            rooz_cfg
        } else {
            None
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

        let labels = Labels::from(&[
            Labels::workspace(&workspace_key),
            Labels::role(labels::WORK_ROLE),
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

        let cli_cfg = self
            .get_cli_config(workspace_key, &cli_config_path, &clone_env)
            .await?;

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
                    (Some((body, _extends_body, format)), _) => {
                        match RoozCfg::deserialize_config(body, *format)? {
                            Some(c) => {
                                if c.bases.is_some() {
                                    return Err("'bases' is not supported in in-repo config (.rooz.yaml); use it in a --config file instead".into());
                                }
                                cfg_builder.from_config(&c);
                                log::debug!("Config file applied.");
                                let origin = format!("{}//.rooz.{}", url, format.to_string());
                                self.config.store(workspace_key, &origin, &body).await?;
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
            )
            .await?;

        if let Some(true) = cli_params.start {
            self.start(&workspace_key).await?;
        }
        Ok(enter_spec)
    }

    pub async fn tmp(&self, spec: &WorkParams, root: bool, shell: &str) -> Result<(), AnyError> {
        let EnterSpec {
            workspace,
            git_spec,
            config,
            ..
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

        let container_id = self
            .enter(
                &workspace.workspace_key,
                working_dir.as_deref(),
                Some(cfg.shell.iter().map(|v| v.as_str()).collect::<Vec<_>>()),
                None,
                root,
            )
            .await?;

        let killed = self.api.container.kill(&container_id, true);
        let volumes = self
            .api
            .volume
            .get_all(&Labels::from(&[Labels::workspace(
                &workspace.workspace_key,
            )]))
            .await?;
        killed.await?;

        let volume_api = self.api.volume;

        let futures = volumes
            .iter()
            .filter_map(|v| Some(async move { volume_api.remove_volume(&v.name, true).await }));
        futures::future::try_join_all(futures).await?;
        Ok(())
    }
}
