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
use colored::Colorize;
use std::collections::HashMap;
use std::fs;

pub const ALLOW_PRIVILEGED_ENV: &str = "ROOZ_ALLOW_PRIVILEGED";

// What the operator has agreed to. Naming the containers keeps the gate useful for
// operators who legitimately need one privileged sidecar: a standing blanket consent
// would wave through a hostile repo config privileging anything else.
#[derive(Debug, PartialEq)]
pub enum PrivilegedConsent {
    None,
    All,
    Named(Vec<String>),
}

impl PrivilegedConsent {
    pub fn resolve(cli_privileged: Option<bool>, env: Option<String>) -> Self {
        // --privileged true is the operator asking for it outright
        if cli_privileged == Some(true) {
            return Self::All;
        }
        match env.as_deref().map(str::trim) {
            None | Some("") | Some("false") => Self::None,
            Some("true") => Self::All,
            Some(list) => Self::Named(
                list.split(',')
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .collect(),
            ),
        }
    }

    fn allows(&self, container: &str) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Named(names) => names.iter().any(|n| n == container),
        }
    }
}

// A privileged container has full access to the host, so the request has to come
// from the operator - config files (in-repo or --config) are authored by whoever
// owns the repository, which is not the same trust level as the operator's machine.
fn check_privileged(containers: &[String], consent: &PrivilegedConsent) -> Result<(), AnyError> {
    let refused = containers
        .iter()
        .filter(|c| !consent.allows(c))
        .cloned()
        .collect::<Vec<_>>();

    if refused.is_empty() {
        return Ok(());
    }

    Err(format!(
        "This configuration requests privileged containers: {}. A privileged container has \
         full access to the host running the container engine. Rooz does not grant that on a \
         config file's say-so. If you trust these specific containers, name them: {}={}. \
         ('{}=true' allows any container the configuration privileges, and '--privileged true' \
         additionally makes the work container privileged - prefer naming them.)",
        refused.join(", "),
        ALLOW_PRIVILEGED_ENV,
        refused.join(","),
        ALLOW_PRIVILEGED_ENV
    )
    .into())
}

// Configuration the operator wrote on their own machine is trusted with secrets;
// anything the repository being opened supplies is not. A remote `--config git:...`
// source is repository-authored too, even though the operator typed the URL.
fn config_is_operator_local(source: &Option<ConfigSource>) -> bool {
    match source {
        Some(ConfigSource::Path { value }) => matches!(value, ConfigPath::File { .. }),
        Some(ConfigSource::Update { origin, .. }) => {
            matches!(ConfigPath::from_str(origin), Ok(ConfigPath::File { .. }))
        }
        None => false,
    }
}

impl<'a> WorkspaceApi<'a> {
    async fn ensure_network(
        &self,
        name: &str,
        internal: bool,
        labels: &Labels,
    ) -> Result<(), AnyError> {
        match self
            .api
            .client
            .create_network(NetworkCreateRequest {
                name: name.to_string(),
                internal: if internal { Some(true) } else { None },
                labels: Some(labels.clone().into()),
                ..Default::default()
            })
            .await
        {
            Ok(_) => Ok(()),
            Err(Error::DockerResponseServerError {
                status_code: 409, ..
            }) => {
                log::debug!("Network already exists: {}. Skipping", name);
                Ok(())
            }
            Err(e) if e.to_string().contains("non-overlapping IPv4 address pool") => {
                Err(format!(
                    "Could not create network '{}': the daemon ran out of address pools. \
                     Rooz creates one network per sidecar plus one per peer relation. \
                     Configure 'default-address-pools' with a smaller subnet size (e.g. \"size\": 24) \
                     in the daemon config to allow more networks. Original error: {}",
                    name, e
                )
                .into())
            }
            Err(e) => Err(e.into()),
        }
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
        secrets_allowed: bool,
    ) -> Result<EnterSpec, AnyError> {
        if let Some(c) = &cli_config {
            cfg_builder.from_config(c);
        }
        cfg_builder.from_cli(cli_params, None);

        // refuse before decrypting: nothing should be in plaintext in this process if the
        // merged configuration is not wholly operator-authored
        if !secrets_allowed && cfg_builder.secrets.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err(crate::config::config::SECRETS_NOT_ALLOWED.into());
        }

        self.config
            .decrypt(
                cfg_builder,
                &self.api.get_system_config().await?.age_identity()?,
            )
            .await?;

        // captured before expansion: these plaintext values get substituted all over the
        // config, and must not reach the persisted copy
        let decrypted_secrets = cfg_builder
            .secrets
            .clone()
            .unwrap_or_default()
            .into_values()
            .collect::<Vec<String>>();

        cfg_builder.expand_vars(secrets_allowed)?;

        let cfg = RuntimeConfig::try_from(&*cfg_builder)?;

        // gate before any image, volume, network or container work happens
        check_privileged(
            &cfg.privileged_containers(),
            &PrivilegedConsent::resolve(
                cli_params.privileged,
                std::env::var(ALLOW_PRIVILEGED_ENV).ok(),
            ),
        )?;

        self.api
            .image
            .ensure(&cfg.image, cli_params.pull_image)
            .await?;

        let volume_specs =
            VolumeApi::create_volume_specs(workspace_key, &cfg.data, &cfg.all_mounts(), true);

        let mounts_all = &cfg
            .mounts
            .iter()
            .map(|(target, source)| (target.to_string(), source.resolve_key(target)))
            .collect::<HashMap<String, String>>();

        let volume_results = self.api.volume.ensure_volumes(&volume_specs).await?;

        let home_dir = format!("/home/{}", &cfg.user);
        let mounts_config = self
            .api
            .volume
            .mounts_with_sources(&volume_specs, mounts_all, true);

        let real_mounts = VolumeApi::real_mounts(mounts_config.clone(), Some(&home_dir));

        let cfg = RuntimeConfig {
            real_mounts: real_mounts.clone(),
            ..cfg.clone()
        };

        let mut cfg2 = cfg.clone();

        let container_mounts = self.api.volume.mounts(&real_mounts).await?;
        for (_, m) in real_mounts.clone() {
            //TODO: when initializing volumes both here in sidecars we should verify
            // if each file exists and if not create them
            if let VolumeResult::Created {} = volume_results[&m.volume_name] {
                self.api
                    .volume
                    .populate_volume(m, Some(work_spec.uid.to_string().parse::<i32>()?))
                    .await?;
            }
        }

        let mut labels = work_spec.labels.clone();

        let egress_network = &constants::egress_network(workspace_key);

        self.ensure_network(egress_network, false, &labels).await?;

        let (pair_keys, peer_keys) = RuntimeConfig::workspace_networks(&cfg2.sidecars);
        let pair_networks = pair_keys
            .iter()
            .map(|s| constants::pair_network(workspace_key, s))
            .collect::<Vec<_>>();
        for n in &pair_networks {
            self.ensure_network(n, true, &labels).await?;
        }
        for (a, b) in &peer_keys {
            self.ensure_network(&constants::peer_network(workspace_key, a, b), true, &labels)
                .await?;
        }

        let cfg2 = self
            .ensure_sidecars(&mut cfg2, workspace_key, force, cli_params.pull_image)
            .await?;

        labels.extend(&[Labels::container(constants::DEFAULT_CONTAINER_NAME)]);

        self.config
            .store_runtime(
                workspace_key,
                &cfg2.mask_secrets(&decrypted_secrets).to_string()?,
            )
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
            additional_networks: if !pair_networks.is_empty() {
                Some(pair_networks.iter().map(|n| n.as_str()).collect())
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
            mounts: container_mounts,
            install: cfg2.install,
            ..*work_spec
        };

        let ws = self.create(&work_spec).await?;
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
                                .map(|(p, b)| {
                                    b.to_string(*format)
                                        .map(|yaml| format!("# {}\n{}", p, yaml))
                                })
                                .collect::<Result<Vec<_>, _>>()?
                                .join("\n---\n");
                            (
                                Some(merged),
                                if bases_yaml.is_empty() {
                                    None
                                } else {
                                    Some(bases_yaml)
                                },
                            )
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
                                    .map(|(path, b)| {
                                        b.to_string(fmt).map(|yaml| format!("# {}\n{}", path, yaml))
                                    })
                                    .collect::<Result<Vec<_>, _>>()?
                                    .join("\n---\n");
                                let base_body = if bases_yaml.is_empty() {
                                    None
                                } else {
                                    Some(bases_yaml)
                                };
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
                            Some(ConfigBody {
                                body,
                                bases,
                                merged,
                            }) => {
                                let fmt = FileFormat::from_path(&file_path);
                                let cfg = merged.map(Ok).unwrap_or_else(|| {
                                    RoozCfg::deserialize_config(&body, fmt).map(|o| o.unwrap())
                                })?;
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
        let mut repo_config_applied = false;
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
                                repo_config_applied = true;
                                eprintln!(
                                    "{}",
                                    "NOTE: applying the configuration provided by this repository (.rooz.yaml). \
                                     It controls the image, mounts and commands of your workspace."
                                        .yellow()
                                );
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
                config_is_operator_local(&cli_config_path) && !repo_config_applied,
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

        let cfg = RuntimeConfig::try_from(&RoozCfg {
            shell: Some(vec![shell.into()]),
            ..config
        })?;

        let container_id = self
            .enter(
                &workspace.workspace_key,
                working_dir.as_deref(),
                Some(cfg.shell.iter().map(|v| v.as_str()).collect::<Vec<_>>()),
                None,
                root,
                false,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::runtime::{RoozSidecarRuntime, RuntimeConfig};
    use std::collections::HashMap;

    fn sidecar(privileged: bool) -> RoozSidecarRuntime {
        let cfg: RoozCfg = serde_yaml::from_str("sidecars:\n  s:\n    image: alpine\n").unwrap();
        let mut s = RuntimeConfig::try_from(&cfg).unwrap().sidecars["s"].clone();
        s.privileged = privileged;
        s
    }

    fn cfg(privileged_work: bool, sidecars: &[(&str, bool)]) -> RuntimeConfig {
        RuntimeConfig {
            privileged: privileged_work,
            sidecars: sidecars
                .iter()
                .map(|(n, p)| (n.to_string(), sidecar(*p)))
                .collect::<HashMap<_, _>>(),
            ..Default::default()
        }
    }

    fn consent(env: Option<&str>) -> PrivilegedConsent {
        PrivilegedConsent::resolve(None, env.map(String::from))
    }

    #[test]
    fn nothing_privileged_needs_no_consent() {
        let c = cfg(false, &[("db", false)]);
        assert!(c.privileged_containers().is_empty());
        assert!(check_privileged(&c.privileged_containers(), &consent(None)).is_ok());
    }

    #[test]
    fn privileged_workspace_and_sidecars_are_all_listed() {
        let c = cfg(true, &[("pwn", true), ("db", false)]);
        assert_eq!(c.privileged_containers(), vec!["pwn", "work"]);
    }

    #[test]
    fn config_requested_privileged_is_refused_without_consent() {
        let c = cfg(false, &[("pwn", true)]);
        let err = check_privileged(&c.privileged_containers(), &consent(None))
            .unwrap_err()
            .to_string();
        assert!(err.contains("pwn"), "offender not named: {}", err);
        assert!(
            err.contains(ALLOW_PRIVILEGED_ENV),
            "no remedy given: {}",
            err
        );
        // an explicit `--privileged false` is not consent either
        let explicit_false = PrivilegedConsent::resolve(Some(false), None);
        assert!(check_privileged(&c.privileged_containers(), &explicit_false).is_err());
        assert!(check_privileged(&c.privileged_containers(), &consent(Some("false"))).is_err());
    }

    #[test]
    fn naming_a_container_consents_to_only_that_container() {
        // the real shape: a trusted overlay privileges `dkr`, the repo's own config must
        // not be able to smuggle in anything else under that standing consent
        let allow_dkr = consent(Some("dkr"));
        assert!(
            check_privileged(
                &cfg(false, &[("dkr", true)]).privileged_containers(),
                &allow_dkr
            )
            .is_ok()
        );

        for hostile in [
            cfg(true, &[("dkr", true)]),
            cfg(false, &[("dkr", true), ("pwn", true)]),
        ] {
            let err = check_privileged(&hostile.privileged_containers(), &allow_dkr)
                .unwrap_err()
                .to_string();
            assert!(
                !err.contains("dkr"),
                "consented container was refused: {}",
                err
            );
        }
    }

    #[test]
    fn named_consent_lists_only_the_unconsented_containers() {
        let c = cfg(true, &[("dkr", true)]);
        let err = check_privileged(&c.privileged_containers(), &consent(Some("dkr")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("work"), "{}", err);
        assert!(
            err.contains(&format!("{}=work", ALLOW_PRIVILEGED_ENV)),
            "remedy should name only what was refused: {}",
            err
        );
    }

    #[test]
    fn consent_parsing() {
        assert_eq!(consent(None), PrivilegedConsent::None);
        assert_eq!(consent(Some("")), PrivilegedConsent::None);
        assert_eq!(consent(Some("false")), PrivilegedConsent::None);
        assert_eq!(consent(Some("true")), PrivilegedConsent::All);
        assert_eq!(
            consent(Some(" dkr , images ,")),
            PrivilegedConsent::Named(vec!["dkr".into(), "images".into()])
        );
        // the CLI flag is an outright request, so it consents to everything
        assert_eq!(
            PrivilegedConsent::resolve(Some(true), None),
            PrivilegedConsent::All
        );
    }

    #[test]
    fn blanket_consent_still_works_for_non_interactive_use() {
        let c = cfg(true, &[("pwn", true)]);
        assert!(check_privileged(&c.privileged_containers(), &consent(Some("true"))).is_ok());
    }
}
