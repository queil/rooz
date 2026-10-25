use crate::api::VolumeApi;
use crate::api::config::{ConfigBody, LocalReader, render_bases, resolve_data_files};
use crate::cmd::plan::{Plan, PlanMode};
use crate::config::trust::TrustedConfigs;
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
        live, progress,
    },
};
use bollard::errors::Error;
use bollard_stubs::models::NetworkCreateRequest;
use bollard_stubs::query_parameters::InspectNetworkOptions;
use std::collections::HashMap;
use std::fs;
use std::str::FromStr;

pub const ALLOW_PRIVILEGED_ENV: &str = "ROOZ_ALLOW_PRIVILEGED";
pub const ALLOW_SECRETS_ENV: &str = "ROOZ_ALLOW_SECRETS";
pub const ALLOW_VPN_ENV: &str = "ROOZ_ALLOW_VPN";
const CONFIG_DOCS: &str = "https://rooz.dev/configuration.html";

// What the operator has agreed to. Naming the containers (or secrets) keeps the gate useful
// for operators who legitimately need one privileged sidecar: a standing blanket consent
// would wave through a hostile config privileging anything else.
#[derive(Debug, PartialEq)]
pub enum Consent {
    None,
    All,
    Named(Vec<String>),
}

impl Consent {
    pub fn resolve(cli: Option<bool>, env: Option<String>) -> Self {
        // the CLI flag set to true is the operator asking for it outright
        if cli == Some(true) {
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

    fn allows(&self, name: &str) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Named(names) => names.iter().any(|n| n == name),
        }
    }

    fn refused(&self, names: &[String]) -> Vec<String> {
        names.iter().filter(|n| !self.allows(n)).cloned().collect()
    }

    // Consent to a grant on a container is pinned to its definition: 'name@fingerprint'. A bare
    // name would also cover whatever another config declares under that name.
    fn allows_pinned(&self, name: &str, fingerprint: &str) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Named(entries) => entries.iter().any(|e| {
                e.split_once('@')
                    .is_some_and(|(n, f)| n == name && f.eq_ignore_ascii_case(fingerprint))
            }),
        }
    }

    // the refused containers as 'name@fingerprint', and the ones consented to by bare name only
    fn refused_pinned(&self, pins: &[(String, String)]) -> (Vec<String>, Vec<String>) {
        let refused = pins
            .iter()
            .filter(|(n, f)| !self.allows_pinned(n, f))
            .collect::<Vec<_>>();
        let unpinned = refused
            .iter()
            .filter(|(n, _)| self.allows(n))
            .map(|(n, _)| n.clone())
            .collect();
        (
            refused
                .iter()
                .map(|(n, f)| format!("{}@{}", n, f))
                .collect(),
            unpinned,
        )
    }
}

fn unpinned_hint(unpinned: &[String]) -> String {
    if unpinned.is_empty() {
        String::new()
    } else {
        format!(" (a bare name no longer consents: {})", unpinned.join(", "))
    }
}

// Which of the operator's secrets may be expanded into this workspace. A local '--config'
// path is not proof of authorship: a file copied from elsewhere can come with instructions to
// pass it ('rooz new ws --config ./setup.yaml'), and that file then picks the
// image, entrypoint and environment the plaintext lands in. So the operator names the secrets,
// the same way they name the containers they allow to run privileged.
fn check_secrets(
    secrets: Option<&indexmap::IndexMap<String, String>>,
    consent: &Consent,
) -> Result<(), AnyError> {
    let refused = consent.refused(
        &secrets
            .map(|s| s.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default(),
    );

    if refused.is_empty() {
        return Ok(());
    }

    Err(format!(
        "This configuration expands secrets: {}. A secret's plaintext ends up inside a container \
         whose image, entrypoint, environment and install steps the same configuration chooses, \
         and a local '--config' path is not proof that you wrote it - repositories do ship config \
         files and ask you to pass them. If this configuration is yours, name the secrets you are \
         releasing to it: {}={}. ('{}=true' releases whatever the configuration declares, and \
         '--allow-secrets true' does the same for a single run.)",
        refused.join(", "),
        ALLOW_SECRETS_ENV,
        refused.join(","),
        ALLOW_SECRETS_ENV
    )
    .into())
}

// A privileged container has full access to the host, so the request has to come
// from the operator - config files (local or git --config) may come from anywhere,
// which is not the same trust level as the operator's machine.
// What 'update' carries into the rebuild: data files are written again from the config, and the
// sidecar images rooz recorded on the containers it removed (by sidecar name) may be reused.
// `pull_sidecar_images` stands apart from the work image's pull, which 'update' does up front.
#[derive(Debug, Default)]
pub struct Rebuild {
    pub sidecar_images: HashMap<String, String>,
    pub pull_sidecar_images: bool,
    // every install step runs, whatever its skip_if says: that state may be in a volume the
    // rebuilt image does not share
    pub run_all_steps: bool,
}

fn check_privileged(containers: &[(String, String)], consent: &Consent) -> Result<(), AnyError> {
    let (refused, unpinned) = consent.refused_pinned(containers);

    if refused.is_empty() {
        return Ok(());
    }

    Err(format!(
        "This configuration requests privileged containers: {}. Allow with {}={}{} - see \
         {}#privileged-containers",
        refused.join(", "),
        ALLOW_PRIVILEGED_ENV,
        refused.join(","),
        unpinned_hint(&unpinned),
        CONFIG_DOCS
    )
    .into())
}

fn check_vpn(containers: &[(String, String)], consent: &Consent) -> Result<(), AnyError> {
    let (refused, unpinned) = consent.refused_pinned(containers);

    if refused.is_empty() {
        return Ok(());
    }

    Err(format!(
        "This configuration requests vpn sidecars: {}. Allow with {}={}{} - see {}#vpn-sidecars",
        refused.join(", "),
        ALLOW_VPN_ENV,
        refused.join(","),
        unpinned_hint(&unpinned),
        CONFIG_DOCS
    )
    .into())
}

pub const EXTRA_REPOS_NOT_ALLOWED: &str = "This configuration lists 'extra_repos', but it is not \
     wholly yours: it comes from a remote '--config <url>//<path>' source, or merges a \
     'bases' layer, not in ROOZ_TRUSTED_CONFIGS. Extra repositories are cloned with your ssh key, so a remote config \
     must not be able to name which of your private repositories land in its workspace. List \
     the source in ROOZ_TRUSTED_CONFIGS, or use a local '--config' file.";

pub const CACHES_NOT_ALLOWED: &str = "This configuration declares shared caches, but it is not \
     wholly yours: it comes from a remote '--config <url>//<path>' source, or merges a \
     'bases' layer, not in ROOZ_TRUSTED_CONFIGS. A cache volume is shared with every other \
     workspace of yours mounting the same path, so whatever this repository's build writes \
     into it (a cargo registry source, a linker config, a crate with a build script) would \
     later execute in those workspaces. Rooz shares only the cache paths you ask for \
     yourself - if you want these shared, list them in ROOZ_CACHES (which also covers \
     'rooz update'), or pass '--caches' to 'rooz new'.";

// Cache volumes are shared by path across all of an operator's workspaces - that is what
// they are for, and why an untrusted config must not be able to opt one of its own
// build paths into that sharing. Only paths the operator named on their own machine (the
// '--caches' flag, the ROOZ_CACHES variable, or a local '--config' file) are shared.
fn operator_caches(cli: &WorkParams, operator_config: Option<&RoozCfg>) -> Vec<String> {
    let mut caches = cli.caches.clone().unwrap_or_default();
    caches.extend(cli.env.caches.clone().unwrap_or_default());
    caches.extend(
        operator_config
            .and_then(|c| c.caches.clone())
            .unwrap_or_default(),
    );
    caches
}

fn check_caches(caches: &[String], consented: &[String]) -> Result<(), AnyError> {
    let refused = caches
        .iter()
        .filter(|c| !consented.contains(c))
        .cloned()
        .collect::<Vec<_>>();

    if refused.is_empty() {
        return Ok(());
    }

    Err(format!(
        "{} Refusing cache paths: {}.",
        CACHES_NOT_ALLOWED,
        refused
            .iter()
            .map(|c| format!("'{}'", c))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .into())
}

// The single gate on configuration the operator did not author. Which fields those may set
// is decided in one place (RoozCfg's operator_only_fields), so a field added to the config
// does not become settable by an untrusted config by default; this function only applies each refused
// field's consent rule.
fn check_untrusted_config(cfg: &RoozCfg, consented_caches: &[String]) -> Result<(), AnyError> {
    for field in crate::config::config::operator_only_fields(cfg) {
        match field {
            "caches" => check_caches(cfg.caches.as_deref().unwrap_or_default(), consented_caches)?,
            // secrets from an untrusted config are never expanded, consent or not
            "secrets" => return Err(crate::config::config::SECRETS_NOT_ALLOWED.into()),
            "extra_repos" => return Err(EXTRA_REPOS_NOT_ALLOWED.into()),
            other => {
                return Err(format!(
                    "Refusing '{}': this configuration comes from a remote source, or merges a \
                     'bases' layer, not in ROOZ_TRUSTED_CONFIGS, and '{}' is the operator's to set.",
                    other, other
                )
                .into());
            }
        }
    }
    Ok(())
}

// A resolved '--config' source. A 'bases' layer is a separate file the operator pointed at -
// a team overlay, something copied from elsewhere - not content they wrote, and the merge
// drops the 'bases' field, so the fact has to travel with the result.
enum NewOutcome {
    Created(EnterSpec),
    Planned(Plan),
}

struct CliConfig {
    cfg: Option<RoozCfg>,
    bases_merged: bool,
    trusted: bool,
    stored: Option<StoredConfig>,
}

struct StoredConfig {
    origin: String,
    body: String,
    bases: Option<String>,
}

// Configuration the operator wrote on their own machine is trusted with secrets;
// a remote `--config git:...` source is not, even though the operator typed the URL,
// unless it is listed in ROOZ_TRUSTED_CONFIGS.
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
                let found = self
                    .api
                    .client
                    .inspect_network(name, None::<InspectNetworkOptions>)
                    .await?
                    .labels
                    .unwrap_or_default();
                if !crate::api::container::same_owner(labels, &found) {
                    return Err(format!(
                        "A network named '{}' already exists and is not this workspace's: \
                         refusing to join it.",
                        name
                    )
                    .into());
                }
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

    // Everything between the parsed configuration and the first engine call: merge, consent,
    // decrypt, expand, gate. Shared by the real run and --dry-run, so a plan refuses exactly
    // what a run would.
    async fn resolve_runtime(
        &self,
        cfg_builder: &mut RoozCfg,
        cli_config: &Option<RoozCfg>,
        cli_params: &WorkParams,
        operator_authored: bool,
    ) -> Result<(RuntimeConfig, Vec<String>), AnyError> {
        if let Some(c) = cli_config {
            cfg_builder.from_config(c);
        }
        cfg_builder.from_cli(cli_params, None);

        // refuse before decrypting: nothing should be in plaintext in this process if the
        // merged configuration is not wholly operator-authored
        if !operator_authored && cfg_builder.secrets.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err(crate::config::config::SECRETS_NOT_ALLOWED.into());
        }

        if !operator_authored {
            let root_steps = cfg_builder.root_install_steps();
            if !root_steps.is_empty() {
                return Err(format!(
                    "{} {}",
                    crate::config::config::ROOT_STEPS_NOT_ALLOWED,
                    root_steps.join(", ")
                )
                .into());
            }
        }

        // provenance says where the configuration came from; consent says whether the operator
        // meant that configuration to have their secrets. Both are needed before decrypting.
        check_secrets(
            cfg_builder.secrets.as_ref(),
            &Consent::resolve(
                cli_params.allow_secrets,
                std::env::var(ALLOW_SECRETS_ENV).ok(),
            ),
        )?;

        self.config
            .decrypt(
                cfg_builder,
                &self.api.get_system_config().await?.age_identity()?,
            )
            .await?;

        // captured before expansion: these plaintext values get substituted all over the
        // config, and must not reach the persisted copy
        let mut decrypted_secrets = cfg_builder
            .secrets
            .clone()
            .unwrap_or_default()
            .into_values()
            .collect::<Vec<String>>();

        cfg_builder.expand_vars(operator_authored)?;
        // a sidecar's egress policy becomes its proxy; the CA key it generates is masked like a secret
        let egress_image = std::env::var(constants::EGRESS_IMAGE_ENV)
            .unwrap_or_else(|_| constants::EGRESS_IMAGE.to_string());
        decrypted_secrets.extend(crate::config::egress::expand(cfg_builder, &egress_image)?);

        let mut cfg = RuntimeConfig::try_from(&*cfg_builder)?;
        // not a config field: the work container's uid is whatever the workspace was made with
        if let Some(uid) = cli_params.uid {
            cfg.uid = uid as i32;
        }

        // gate before any image, volume, network or container work happens
        let masked = cfg.mask_secrets(&decrypted_secrets);
        let pins = |names: Vec<String>| {
            names
                .into_iter()
                .map(|n| {
                    let f = masked.fingerprint(&n);
                    (n, f)
                })
                .collect::<Vec<_>>()
        };
        check_privileged(
            &pins(cfg.privileged_containers()),
            &Consent::resolve(
                cli_params.privileged,
                std::env::var(ALLOW_PRIVILEGED_ENV).ok(),
            ),
        )?;
        check_vpn(
            &pins(cfg.vpn_containers()),
            &Consent::resolve(None, std::env::var(ALLOW_VPN_ENV).ok()),
        )?;

        Ok((cfg, decrypted_secrets))
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
        stored_config: Option<StoredConfig>,
        operator_authored: bool,
        rebuild: Option<&Rebuild>,
    ) -> Result<EnterSpec, AnyError> {
        let (cfg, decrypted_secrets) = self
            .resolve_runtime(cfg_builder, &cli_config, cli_params, operator_authored)
            .await?;

        // pull first: a failed pull must not leave a stored config behind
        progress::phase("image", &cfg.image);
        self.api
            .image
            .ensure(&cfg.image, cli_params.pull_image)
            .await?;

        if let Some(StoredConfig {
            origin,
            body,
            bases,
        }) = stored_config
        {
            self.config.store(workspace_key, &origin, &body).await?;
            if let Some(bases) = bases {
                self.config.store_bases(workspace_key, &bases).await?;
            }
        }

        let volume_specs =
            VolumeApi::create_volume_specs(workspace_key, &cfg.data, &cfg.all_mounts(), true)?;

        let mounts_all = &cfg
            .mounts
            .iter()
            .map(|(target, source)| {
                (
                    target.to_string(),
                    source.resolve_key(constants::DEFAULT_CONTAINER_NAME, target),
                )
            })
            .collect::<HashMap<String, String>>();

        let volume_results = self.api.volume.ensure_volumes(&volume_specs).await?;
        progress::phase(
            "volumes",
            format!(
                "{} ready, {} created",
                volume_results.len(),
                volume_results
                    .values()
                    .filter(|r| matches!(r, VolumeResult::Created))
                    .count()
            ),
        );

        let home_dir = format!("/home/{}", cfg.user);
        let mounts_config = self
            .api
            .volume
            .mounts_with_sources(&volume_specs, mounts_all, true)?;

        let real_mounts = VolumeApi::real_mounts(mounts_config.clone(), Some(&home_dir));

        let cfg = RuntimeConfig {
            real_mounts: real_mounts.clone(),
            ..cfg.clone()
        };

        let cfg2 = cfg.clone();

        let container_mounts = self.api.volume.mounts(&real_mounts).await?;
        for (_, m) in real_mounts.clone() {
            let created = matches!(volume_results[&m.volume_name], VolumeResult::Created);
            if created || !m.files.is_empty() {
                self.api
                    .volume
                    .populate_volume(
                        m,
                        Some(work_spec.uid.to_string().parse::<i32>()?),
                        created,
                        rebuild.is_some(),
                    )
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

        let sidecars_input = cfg2.clone();
        labels.extend(&[Labels::container(constants::DEFAULT_CONTAINER_NAME)]);

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
            args: (if !cfg2.args.is_empty() {
                Some(&cfg2.args)
            } else {
                None
            })
            .as_ref()
            .map(|x| x.iter().map(|z| z.as_ref()).collect()),
            command: (if !cfg2.command.is_empty() {
                Some(&cfg2.command)
            } else {
                None
            })
            .as_ref()
            .map(|x| x.iter().map(|z| z.as_ref()).collect()),
            mounts: container_mounts,
            install: cfg2.install,
            run_all_steps: rebuild.is_some_and(|r| r.run_all_steps),
            ..*work_spec
        };

        progress::phase("container", work_spec.container_name);
        // the sidecars and the work container are independent until they start: their images
        // build and their install steps run side by side
        live::begin();
        // both run to the end even when the other fails, so neither leaves half-done work behind
        let (sidecars_result, work_result) = futures::join!(
            self.ensure_sidecars(
                &sidecars_input,
                workspace_key,
                force,
                cli_params.pull_image || rebuild.is_some_and(|r| r.pull_sidecar_images),
                rebuild.is_some(),
                rebuild.is_some_and(|r| r.run_all_steps),
                rebuild.map(|r| &r.sidecar_images),
            ),
            self.create(&work_spec),
        );
        live::end();
        let (sidecars_cfg, ws) = match (sidecars_result, work_result) {
            (Ok(sidecars_cfg), Ok(ws)) => (sidecars_cfg, ws),
            (sidecars, work) => {
                return Err(
                    crate::api::sidecar::all_ok([sidecars.map(|_| ()), work.map(|_| ())])
                        .unwrap_err(),
                );
            }
        };

        self.config
            .store_runtime(
                workspace_key,
                &sidecars_cfg.mask_secrets(&decrypted_secrets).to_string()?,
            )
            .await?;
        if !cfg2.extra_repos.is_empty() {
            progress::phase("clone", format!("{} extra repo(s)", cfg2.extra_repos.len()));
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
        cli_config_path: &Option<ConfigSource>,
        clone_env: &CloneEnv,
        secrets_consent: &Consent,
        trusted_configs: &TrustedConfigs,
    ) -> Result<CliConfig, AnyError> {
        let val = if let Some(source) = &cli_config_path {
            let mut trusted: bool;
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
                    let body = value.to_string(*format)?;
                    let reader = LocalReader::default();
                    let mut value = value.clone();
                    if let Ok(ConfigPath::File { path }) = ConfigPath::from_str(origin) {
                        resolve_data_files(&reader, &path, &mut value).await?;
                    }
                    let value = &value;
                    trusted = match ConfigPath::from_str(origin)? {
                        ConfigPath::File { path } => trusted_configs.trusts_files([path.as_str()]),
                        ConfigPath::Git { url, .. } => trusted_configs.trusts_repo(&url),
                    };
                    let (cfg, base_body) = if value.bases.is_some() {
                        if let Ok(ConfigPath::File { path }) = ConfigPath::from_str(origin) {
                            let (merged, individual_bases) = self
                                .config
                                .resolve_extends_chain(&reader, &path, value.clone(), 0)
                                .await?;
                            (Some(merged), render_bases(&individual_bases, *format)?)
                        } else {
                            (Some(value.clone()), None)
                        }
                    } else {
                        (Some(value.clone()), None)
                    };
                    trusted = trusted
                        && trusted_configs
                            .trusts_files(reader.read.lock().unwrap().iter().map(String::as_str));
                    (origin.to_string(), Some(body), base_body, cfg)
                }
                ConfigSource::Path { value: path } => match path {
                    ConfigPath::File { path } => {
                        let body = fs::read_to_string(path)?;
                        let absolute_path =
                            std::path::absolute(path)?.to_string_lossy().into_owned();
                        let fmt = FileFormat::from_path(path)?;
                        let mut c = RoozCfg::deserialize_config(&body, fmt)?;
                        let reader = LocalReader::default();
                        resolve_data_files(&reader, path, &mut c).await?;

                        let (cfg, base_body) = if c.bases.is_some() {
                            let (merged, individual_bases) = self
                                .config
                                .resolve_extends_chain(&reader, path.as_str(), c, 0)
                                .await?;
                            (Some(merged), render_bases(&individual_bases, fmt)?)
                        } else {
                            (Some(c), None)
                        };
                        trusted = trusted_configs.trusts_files(
                            std::iter::once(path.as_str())
                                .chain(reader.read.lock().unwrap().iter().map(String::as_str)),
                        );
                        (absolute_path, Some(body.clone()), base_body, cfg)
                    }
                    ConfigPath::Git { url, file_path } => {
                        trusted = trusted_configs.trusts_repo(url);
                        let (result, _) = self
                            .git
                            .clone_config_repo(clone_env.clone(), url, file_path)
                            .await?;

                        let (rooz_cfg, main_body, base_body) = match result {
                            Some(ConfigBody {
                                body,
                                bases,
                                merged,
                            }) => {
                                let fmt = FileFormat::from_path(file_path)?;
                                let cfg = match merged {
                                    Some(merged) => merged,
                                    None => RoozCfg::deserialize_config(&body, fmt)?,
                                };
                                (Some(cfg), Some(body), bases)
                            }
                            None => (None, None, None),
                        };

                        (path.to_string(), main_body, base_body, rooz_cfg)
                    }
                },
            };

            // before the first write: a refusal must not leave a half-created workspace behind,
            // or the operator cannot re-run the same command once they have consented
            check_secrets(
                rooz_cfg.as_ref().and_then(|c| c.secrets.as_ref()),
                secrets_consent,
            )?;

            CliConfig {
                bases_merged: extends_body.is_some(),
                trusted,
                cfg: rooz_cfg,
                stored: Some(StoredConfig {
                    origin,
                    body: body.unwrap(),
                    bases: Some(extends_body.unwrap_or_default()),
                }),
            }
        } else {
            CliConfig {
                cfg: None,
                bases_merged: false,
                trusted: false,
                stored: None,
            }
        };

        Ok(val)
    }

    pub async fn new(
        &self,
        workspace_key: &str,
        cli_params: &WorkParams,
        cli_config_path: Option<ConfigSource>,
        ephemeral: bool,
        rebuild: Option<Rebuild>,
    ) -> Result<EnterSpec, AnyError> {
        self.api.volume.require_ssh_key().await?;
        match self
            .new_inner(
                workspace_key,
                cli_params,
                cli_config_path,
                ephemeral,
                rebuild.as_ref(),
                None,
            )
            .await?
        {
            NewOutcome::Created(spec) => Ok(spec),
            NewOutcome::Planned(_) => unreachable!("a plan is only produced when one is asked for"),
        }
    }

    // Resolves and gates the configuration exactly as 'new' does, then reports what a run would
    // do instead of doing it. It reads the engine (and, for 'git:' sources, fetches the config
    // with a short-lived helper container); it creates no workspace resources and clones no
    // repository.
    pub async fn plan_new(
        &self,
        workspace_key: &str,
        cli_params: &WorkParams,
        cli_config_path: Option<ConfigSource>,
        mode: PlanMode,
    ) -> Result<Plan, AnyError> {
        match self
            .new_inner(
                workspace_key,
                cli_params,
                cli_config_path,
                false,
                None,
                Some(mode),
            )
            .await?
        {
            NewOutcome::Planned(plan) => Ok(plan),
            NewOutcome::Created(_) => unreachable!("a run is never made when a plan is asked for"),
        }
    }

    async fn new_inner(
        &self,
        workspace_key: &str,
        cli_params: &WorkParams,
        cli_config_path: Option<ConfigSource>,
        ephemeral: bool,
        rebuild: Option<&Rebuild>,
        plan: Option<PlanMode>,
    ) -> Result<NewOutcome, AnyError> {
        let orig_uid = cli_params
            .uid
            .map(|x| x.to_string())
            .unwrap_or(constants::DEFAULT_UID.to_string());

        let labels = Labels::from(&[
            Labels::workspace(workspace_key),
            Labels::role(labels::WORK_ROLE),
        ]);

        self.api
            .image
            .ensure(
                constants::HELPER_IMAGE,
                cli_params.pull_image && plan.is_none(),
            )
            .await?;

        let work_dir = constants::WORK_DIR;

        let clone_env = CloneEnv {
            uid: orig_uid.to_string(),
            workspace_key: workspace_key.to_string(),
            working_dir: work_dir.to_string(),
            ..Default::default()
        };

        let CliConfig {
            cfg: cli_cfg,
            bases_merged,
            trusted,
            stored: stored_config,
        } = self
            .get_cli_config(
                &cli_config_path,
                &clone_env,
                &Consent::resolve(
                    cli_params.allow_secrets,
                    std::env::var(ALLOW_SECRETS_ENV).ok(),
                ),
                &TrustedConfigs::from_env(),
            )
            .await?;

        let operator_local = config_is_operator_local(&cli_config_path) || trusted;
        // base layers are merged content the operator did not write, so they carry
        // the same weight as a remote config here
        let operator_authored = operator_local && (trusted || !bases_merged);
        let consented_caches = operator_caches(
            cli_params,
            if operator_authored {
                cli_cfg.as_ref()
            } else {
                None
            },
        );

        // a '--config git:...' source is remote configuration, however the operator
        // typed its URL; a local file with untrusted bases is not wholly theirs either
        if !operator_authored && let Some(c) = &cli_cfg {
            check_untrusted_config(c, &consented_caches)?;
        }

        let work_spec = WorkSpec {
            uid: &orig_uid,
            container_working_dir: work_dir,
            container_name: workspace_key,
            workspace_key,
            ephemeral,
            force_recreate: false,
            ..Default::default()
        };
        let mut cfg_builder = RoozCfg::default().from_cli_env(cli_params.clone());

        if let Some(mode) = &plan {
            let (cfg, secrets) = self
                .resolve_runtime(&mut cfg_builder, &cli_cfg, cli_params, operator_authored)
                .await?;
            return Ok(NewOutcome::Planned(
                self.plan(workspace_key, &cfg, &secrets, mode, cli_params.pull_image)
                    .await?,
            ));
        }

        let root_repo_result = match &RoozCfg::git_ssh_url(cli_params, &cli_cfg) {
            Some(url) => {
                progress::phase("clone", url);
                Some(self.git.clone_root_repo(url, &clone_env).await?)
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
                workspace_key,
                false,
                stored_config,
                operator_authored,
                rebuild,
            )
            .await?;

        if let Some(true) = cli_params.start {
            self.start(workspace_key).await?;
        }
        Ok(NewOutcome::Created(enter_spec))
    }

    pub async fn tmp(&self, spec: &WorkParams, root: bool, shell: &str) -> Result<(), AnyError> {
        let EnterSpec {
            workspace,
            git_spec,
            config,
            ..
        } = self
            .new(&id::random_suffix("tmp"), spec, None, true, None)
            .await?;

        let working_dir = git_spec
            .map(|v| v.dir.to_string())
            .or(Some(workspace.working_dir));

        let cfg = RuntimeConfig::try_from(&RoozCfg {
            shell: Some(vec![shell.into()]),
            ..config
        })?;

        let entered = self
            .enter(
                &workspace.workspace_key,
                working_dir.as_deref(),
                Some(cfg.shell.iter().map(|v| v.as_str()).collect::<Vec<_>>()),
                None,
                root,
                false,
            )
            .await;

        // the workspace is throw-away: it goes away even when the shell could not be entered
        let container_id = match entered {
            Ok(id) => id,
            Err(e) => {
                if let Err(cleanup) = self.remove(&workspace.workspace_key, false, true).await {
                    eprintln!(
                        "WARN: could not remove the throw-away workspace '{}': {}",
                        workspace.workspace_key, cleanup
                    );
                }
                return Err(e);
            }
        };

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
            .map(|v| async move { volume_api.remove_volume(&v.name, true).await });
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

    fn consent(env: Option<&str>) -> Consent {
        Consent::resolve(None, env.map(String::from))
    }

    fn pinned(c: &RuntimeConfig, names: Vec<String>) -> Vec<(String, String)> {
        names
            .into_iter()
            .map(|n| {
                let f = c.fingerprint(&n);
                (n, f)
            })
            .collect()
    }

    fn privileged(c: &RuntimeConfig) -> Vec<(String, String)> {
        pinned(c, c.privileged_containers())
    }

    fn vpn(c: &RuntimeConfig) -> Vec<(String, String)> {
        pinned(c, c.vpn_containers())
    }

    // consent pinned to these containers' current definitions in `c`
    fn pin(c: &RuntimeConfig, names: &[&str]) -> Consent {
        consent(Some(
            &names
                .iter()
                .map(|n| format!("{}@{}", n, c.fingerprint(n)))
                .collect::<Vec<_>>()
                .join(","),
        ))
    }

    fn caches(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn contained_fields_are_settable_by_untrusted_config() {
        // everything an untrusted config may set: it only shapes the workspace's own
        // container
        let cfg: RoozCfg = serde_yaml::from_str(
            "image: alpine\nshell: [sh]\nuser: someone\nports: ['8080:8080']\n\
             env: {A: b}\ninstall: 'echo hi'\ncommand: [sh]\nargs: [-c]\n\
             vars: {v: '1'}\ninit: true\n\
             sidecars:\n  s:\n    image: alpine\n",
        )
        .unwrap();
        assert!(crate::config::config::operator_only_fields(&cfg).is_empty());
        assert!(check_untrusted_config(&cfg, &[]).is_ok());
    }

    #[test]
    fn untrusted_extra_repos_are_refused() {
        let cfg: RoozCfg = serde_yaml::from_str("extra_repos: ['git@h:a/b.git']\n").unwrap();
        assert_eq!(
            crate::config::config::operator_only_fields(&cfg),
            vec!["extra_repos"]
        );
        let err = check_untrusted_config(&cfg, &[]).unwrap_err().to_string();
        assert!(err.contains("ROOZ_TRUSTED_CONFIGS"), "{}", err);
    }

    #[test]
    fn untrusted_secrets_are_refused() {
        let cfg: RoozCfg = serde_yaml::from_str("secrets:\n  token: age-encrypted\n").unwrap();
        assert_eq!(
            crate::config::config::operator_only_fields(&cfg),
            vec!["secrets"]
        );
        assert!(check_untrusted_config(&cfg, &[]).is_err());
    }

    #[test]
    fn untrusted_caches_follow_the_consent_rule() {
        let cfg: RoozCfg = serde_yaml::from_str("caches:\n  - ~/.cargo\n").unwrap();
        assert_eq!(
            crate::config::config::operator_only_fields(&cfg),
            vec!["caches"]
        );
        assert!(check_untrusted_config(&cfg, &[]).is_err());
        assert!(check_untrusted_config(&cfg, &caches(&["~/.cargo"])).is_ok());
    }

    #[test]
    fn untrusted_caches_are_refused_without_consent() {
        let err = check_caches(&caches(&["~/.cargo"]), &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("~/.cargo"), "path not named: {}", err);
        assert!(err.contains("--caches"), "no remedy given: {}", err);
        // consenting to one path is not consent for another
        assert!(check_caches(&caches(&["~/.cargo"]), &caches(&["~/.nuget"])).is_err());
    }

    #[test]
    fn caches_the_operator_asked_for_are_shared() {
        assert!(check_caches(&caches(&["~/.cargo"]), &caches(&["~/.cargo"])).is_ok());
        assert!(check_caches(&caches(&["~/.cargo"]), &caches(&["~/.nuget", "~/.cargo"])).is_ok());
        assert!(check_caches(&[], &[]).is_ok());
    }

    #[test]
    fn consent_comes_from_the_operator_flag_env_and_local_config() {
        let cli = WorkParams {
            caches: Some(caches(&["~/.cargo"])),
            env: crate::cli::WorkEnvParams {
                caches: Some(caches(&["~/.nuget"])),
                ..Default::default()
            },
            ..Default::default()
        };
        let local_config = RoozCfg {
            caches: Some(caches(&["~/.npm"])),
            ..Default::default()
        };
        let consented = operator_caches(&cli, Some(&local_config));
        assert!(check_caches(&caches(&["~/.cargo", "~/.nuget", "~/.npm"]), &consented).is_ok());
        // the same config coming from a remote source (not passed as operator-local) is not consent
        let without = operator_caches(&cli, None);
        assert!(check_caches(&caches(&["~/.npm"]), &without).is_err());
    }

    #[test]
    fn nothing_privileged_needs_no_consent() {
        let c = cfg(false, &[("db", false)]);
        assert!(c.privileged_containers().is_empty());
        assert!(check_privileged(&privileged(&c), &consent(None)).is_ok());
    }

    #[test]
    fn privileged_workspace_and_sidecars_are_all_listed() {
        let c = cfg(true, &[("pwn", true), ("db", false)]);
        assert_eq!(c.privileged_containers(), vec!["pwn", "work"]);
    }

    #[test]
    fn config_requested_privileged_is_refused_without_consent() {
        let c = cfg(false, &[("pwn", true)]);
        let err = check_privileged(&privileged(&c), &consent(None))
            .unwrap_err()
            .to_string();
        assert!(err.contains("pwn"), "offender not named: {}", err);
        assert!(
            err.contains(ALLOW_PRIVILEGED_ENV),
            "no remedy given: {}",
            err
        );
        // an explicit `--privileged false` is not consent either
        let explicit_false = Consent::resolve(Some(false), None);
        assert!(check_privileged(&privileged(&c), &explicit_false).is_err());
        assert!(check_privileged(&privileged(&c), &consent(Some("false"))).is_err());
    }

    #[test]
    fn pinned_consent_covers_only_that_container() {
        // the real shape: a trusted overlay privileges `dkr`, a remote config must
        // not be able to smuggle in anything else under that standing consent
        let ok = cfg(false, &[("dkr", true)]);
        let allow_dkr = pin(&ok, &["dkr"]);
        assert!(check_privileged(&privileged(&ok), &allow_dkr).is_ok());

        for hostile in [
            cfg(true, &[("dkr", true)]),
            cfg(false, &[("dkr", true), ("pwn", true)]),
        ] {
            let err = check_privileged(&privileged(&hostile), &allow_dkr)
                .unwrap_err()
                .to_string();
            assert!(
                !err.contains("dkr@"),
                "consented container was refused: {}",
                err
            );
        }
    }

    #[test]
    fn a_redefined_container_needs_consent_again() {
        // a config declaring its own `dkr` must not inherit consent given to another `dkr`
        let reviewed = cfg(false, &[("dkr", true)]);
        let allow_dkr = pin(&reviewed, &["dkr"]);
        let mut hostile = reviewed.clone();
        hostile.sidecars.get_mut("dkr").unwrap().image = "evil/root:latest".into();
        let err = check_privileged(&privileged(&hostile), &allow_dkr)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(&format!("dkr@{}", hostile.fingerprint("dkr"))),
            "{}",
            err
        );
    }

    #[test]
    fn a_bare_name_no_longer_consents_and_says_why() {
        let c = cfg(false, &[("dkr", true)]);
        let err = check_privileged(&privileged(&c), &consent(Some("dkr")))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("a bare name no longer consents: dkr"),
            "{}",
            err
        );
        assert!(
            err.contains(&format!("{}=dkr@", ALLOW_PRIVILEGED_ENV)),
            "{}",
            err
        );
    }

    #[test]
    fn the_fingerprint_follows_the_definition_not_the_secrets() {
        let mut a = cfg(false, &[("dkr", true)]);
        a.data
            .insert("conf".into(), serde_yaml::from_str("content: one").unwrap());
        a.sidecars
            .get_mut("dkr")
            .unwrap()
            .mounts
            .insert("/etc/conf".into(), serde_yaml::from_str("conf").unwrap());
        a.sidecars
            .get_mut("dkr")
            .unwrap()
            .env
            .insert("TOKEN".into(), "s3cret".into());
        let mut rotated = a.clone();
        rotated
            .sidecars
            .get_mut("dkr")
            .unwrap()
            .env
            .insert("TOKEN".into(), "n3w".into());
        assert_eq!(
            a.mask_secrets(&["s3cret".into()]).fingerprint("dkr"),
            rotated.mask_secrets(&["n3w".into()]).fingerprint("dkr"),
        );
        // the data behind a mount is part of what the container runs
        let mut changed = a.clone();
        changed
            .data
            .insert("conf".into(), serde_yaml::from_str("content: two").unwrap());
        assert_ne!(a.fingerprint("dkr"), changed.fingerprint("dkr"));
        assert_eq!(a.fingerprint("dkr"), a.clone().fingerprint("dkr"));
    }

    #[test]
    fn named_consent_lists_only_the_unconsented_containers() {
        let c = cfg(true, &[("dkr", true)]);
        let err = check_privileged(&privileged(&c), &pin(&c, &["dkr"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("work"), "{}", err);
        assert!(
            err.contains(&format!(
                "{}=work@{}",
                ALLOW_PRIVILEGED_ENV,
                c.fingerprint("work")
            )),
            "remedy should name only what was refused: {}",
            err
        );
    }

    #[test]
    fn consent_parsing() {
        assert_eq!(consent(None), Consent::None);
        assert_eq!(consent(Some("")), Consent::None);
        assert_eq!(consent(Some("false")), Consent::None);
        assert_eq!(consent(Some("true")), Consent::All);
        assert_eq!(
            consent(Some(" dkr , images ,")),
            Consent::Named(vec!["dkr".into(), "images".into()])
        );
        // the CLI flag is an outright request, so it consents to everything
        assert_eq!(Consent::resolve(Some(true), None), Consent::All);
    }

    #[test]
    fn blanket_consent_still_works_for_non_interactive_use() {
        let c = cfg(true, &[("pwn", true)]);
        assert!(check_privileged(&privileged(&c), &consent(Some("true"))).is_ok());
    }

    fn vpn_cfg(sidecars: &[(&str, bool)]) -> RuntimeConfig {
        RuntimeConfig {
            sidecars: sidecars
                .iter()
                .map(|(n, v)| {
                    let mut s = sidecar(false);
                    s.vpn = *v;
                    (n.to_string(), s)
                })
                .collect::<HashMap<_, _>>(),
            ..Default::default()
        }
    }

    #[test]
    fn vpn_sidecars_are_listed() {
        let c = vpn_cfg(&[("wg", true), ("db", false), ("ovpn", true)]);
        assert_eq!(c.vpn_containers(), vec!["ovpn", "wg"]);
        assert!(vpn_cfg(&[("db", false)]).vpn_containers().is_empty());
        assert!(check_vpn(&[], &consent(None)).is_ok());
    }

    #[test]
    fn config_requested_vpn_is_refused_without_consent() {
        let c = vpn_cfg(&[("wg", true)]);
        let err = check_vpn(&vpn(&c), &consent(None)).unwrap_err().to_string();
        assert!(err.contains("wg"), "offender not named: {}", err);
        assert!(
            err.contains(&format!("{}=wg", ALLOW_VPN_ENV)),
            "no remedy given: {}",
            err
        );
        assert!(check_vpn(&vpn(&c), &consent(Some("false"))).is_err());
    }

    #[test]
    fn pinning_a_vpn_sidecar_consents_to_only_that_sidecar() {
        let wg = vpn_cfg(&[("wg", true)]);
        let allow_wg = pin(&wg, &["wg"]);
        assert!(check_vpn(&vpn(&wg), &allow_wg).is_ok());
        let both = vpn_cfg(&[("wg", true), ("pwn", true)]);
        let err = check_vpn(&vpn(&both), &allow_wg).unwrap_err().to_string();
        assert!(
            err.contains(&format!("{}=pwn@", ALLOW_VPN_ENV)),
            "remedy should name only what was refused: {}",
            err
        );
        assert!(!err.contains("wg@"), "{}", err);
        assert!(check_vpn(&vpn(&wg), &consent(Some("true"))).is_ok());
    }

    #[test]
    fn privileged_and_vpn_consent_are_separate() {
        // a sidecar can be both; each grant needs its own consent
        let mut c = vpn_cfg(&[("wg", true)]);
        c.sidecars.get_mut("wg").unwrap().privileged = true;
        assert_eq!(c.privileged_containers(), vec!["wg"]);
        assert_eq!(c.vpn_containers(), vec!["wg"]);
    }

    fn secrets(keys: &[&str]) -> indexmap::IndexMap<String, String> {
        keys.iter()
            .map(|k| (k.to_string(), "age-encrypted".to_string()))
            .collect()
    }

    fn secrets_consent(env: Option<&str>) -> Consent {
        Consent::resolve(None, env.map(String::from))
    }

    #[test]
    fn a_config_without_secrets_needs_no_consent() {
        assert!(check_secrets(None, &secrets_consent(None)).is_ok());
        assert!(check_secrets(Some(&secrets(&[])), &secrets_consent(None)).is_ok());
    }

    #[test]
    fn a_local_config_file_is_not_consent_on_its_own() {
        // the bypass: a file copied from elsewhere comes with instructions to pass it with --config
        let err = check_secrets(Some(&secrets(&["opSecret"])), &secrets_consent(None))
            .unwrap_err()
            .to_string();
        assert!(err.contains("opSecret"), "secret not named: {}", err);
        assert!(err.contains(ALLOW_SECRETS_ENV), "no remedy given: {}", err);
        // an explicit refusal is not consent either
        assert!(
            check_secrets(
                Some(&secrets(&["opSecret"])),
                &secrets_consent(Some("false"))
            )
            .is_err()
        );
        assert!(
            check_secrets(
                Some(&secrets(&["opSecret"])),
                &Consent::resolve(Some(false), None)
            )
            .is_err()
        );
    }

    #[test]
    fn naming_a_secret_consents_to_only_that_secret() {
        let allow_db = secrets_consent(Some("dbPassword"));
        assert!(check_secrets(Some(&secrets(&["dbPassword"])), &allow_db).is_ok());

        let err = check_secrets(Some(&secrets(&["dbPassword", "opSecret"])), &allow_db)
            .unwrap_err()
            .to_string();
        assert!(
            !err.contains("dbPassword"),
            "consented secret was refused: {}",
            err
        );
        assert!(
            err.contains(&format!("{}=opSecret", ALLOW_SECRETS_ENV)),
            "remedy should name only what was refused: {}",
            err
        );
    }

    #[test]
    fn secrets_consent_parsing() {
        assert_eq!(secrets_consent(None), Consent::None);
        assert_eq!(secrets_consent(Some("")), Consent::None);
        assert_eq!(secrets_consent(Some("false")), Consent::None);
        assert_eq!(secrets_consent(Some("true")), Consent::All);
        assert_eq!(
            secrets_consent(Some(" dbPassword , token ,")),
            Consent::Named(vec!["dbPassword".into(), "token".into()])
        );
        assert_eq!(Consent::resolve(Some(true), None), Consent::All);
        // blanket consent for CI
        assert!(check_secrets(Some(&secrets(&["a", "b"])), &secrets_consent(Some("true"))).is_ok());
    }
}
