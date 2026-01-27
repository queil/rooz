use crate::config::config::{DataEntry, DataExt};
use crate::model::types::VolumeSpec;
use crate::util::labels::DATA_ROLE;
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
use bollard_stubs::models::Mount;
use bollard_stubs::models::MountTypeEnum::VOLUME;
use std::collections::HashMap;
use std::fs;

impl<'a> WorkspaceApi<'a> {
    fn expand_home(path: String, home: Option<&str>) -> String {
        match (home, path.strip_prefix("~/")) {
            (Some(h), Some(rest)) => format!("{}/{}", h, rest),
            (Some(h), None) if path == "~" => h.to_string(),
            _ => path.clone(),
        }
    }
    async fn volumes_v2(
        &self,
        workspace_key: &str,
        config: &RuntimeConfig,
    ) -> Result<(Vec<VolumeSpec>, Vec<Mount>), AnyError> {
        let mut data_entries = vec![];
        data_entries.extend_from_slice(config.data.clone().into_entries().as_slice());
        let volumes_v2 = data_entries
            .iter()
            .map(|d| match d {
                DataEntry::Dir { name } => VolumeSpec {
                    name: format!(
                        "rooz-{}-{}",
                        id::sanitize(workspace_key),
                        id::sanitize(name)
                    ),
                    labels: Some(Labels::from(&[
                        Labels::workspace(workspace_key),
                        Labels::role(DATA_ROLE),
                    ])),
                },
                _ => panic!("Not implemented yet"),
            })
            .collect::<Vec<_>>();

        for v in volumes_v2.iter().clone() {
            self.api.volume.ensure_volume_v2(&v).await?;
        }

        let home_dir = format!("/home/{}", &config.user);

        let mut mount_entries = HashMap::new();

        mount_entries.extend(config.mounts.clone());

        let mounts_v2 = {
            let unknown_entries: Vec<_> = mount_entries
                .values()
                .filter(|k| !data_entries.clone().into_iter().any(|e| e.name() == *k))
                .collect();

            if !unknown_entries.is_empty() {
                panic!(
                    "Invalid mounts spec. The following entries must be declared under data: {}",
                    unknown_entries
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }

            mount_entries
                .into_iter()
                .map(|(target, source)| Mount {
                    target: Some(Self::expand_home(target, Some(&home_dir))),
                    source: Some(format!(
                        "rooz-{}-{}",
                        id::sanitize(workspace_key),
                        id::sanitize(&source)
                    )),
                    typ: Some(VOLUME),
                    read_only: Some(false),
                    ..Mount::default()
                })
                .collect::<Vec<_>>()
        };

        Ok((volumes_v2, mounts_v2))
        //TODO: initialize volumes according to the DataEntry type

        // TODO: DESIGN CHANGES - BREAKING
        // /work is not longer backed by a volume by default
        // In volumes-v2 it can be explicitly configured for workspaces with configuration files, but
        // can't in tmp or simple persistent workspaces without config

        //TODO: NEXT STEPS

        //TODO REFAC BUG - when making mounts_v2 - the source must be the actual volume name, not the data
        // entry name. I fixed it but the logic for generating volume name from the daate entry name
        // should be unified

        //TODO: 3. all built-in stuff must be included in v2 - caches, ssh, system-config, etc.

        //TODO: 4. v2 in sidecars
        // - remove mount_work

        //TODO 5. caches and system shared volumes (ssh-key) shall maybe owned by a rooz group that need to be
        // ensured in containers and the user would beed to be added to that group to read (and write as the group - caches)

        // ---- END VOLUMES v2 impl ----
    }

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

        //TODO: v2 - do not forget that in DataEntry::Image init
        // if let Some(home_from_image) = &cfg.home_from_image {
        //     self.api
        //         .image
        //         .ensure(&home_from_image, cli_params.pull_image)
        //         .await?;
        // }

        let (volumes_v2, mounts_v2) = self.volumes_v2(workspace_key, &cfg).await?;

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

        labels.extend(&[Labels::container(constants::DEFAULT_CONTAINER_NAME)]);

        self.config
            .store_runtime(workspace_key, &cfg.clone().to_string()?)
            .await?;

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
            init: cfg.init,
            args: (if cfg.args.len() > 0 {
                Some(&cfg.args)
            } else {
                None
            })
            .as_ref()
            .map(|x| x.iter().map(|z| z.as_ref()).collect()),
            command: (if cfg.command.len() > 0 {
                Some(&cfg.command)
            } else {
                None
            })
            .as_ref()
            .map(|x| x.iter().map(|z| z.as_ref()).collect()),
            mounts: mounts_v2,
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
    ) -> Result<Option<RoozCfg>, AnyError> {
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

            self.config
                .store(workspace_key, &origin, &body.unwrap())
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
        // volume v2 test

        // end: volume v2 test

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
                    (Some((body, format)), _) => {
                        match RoozCfg::deserialize_config(body, *format)? {
                            Some(c) => {
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

        let container_id = self
            .enter(
                &workspace.workspace_key,
                working_dir.as_deref(),
                Some(cfg.shell.iter().map(|v| v.as_str()).collect::<Vec<_>>()),
                None,
                &workspace.orig_uid,
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
