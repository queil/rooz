use age::x25519::Identity;
use bollard::{
    network::ListNetworksOptions,
    service::{ContainerSummary, Volume},
    volume::ListVolumesOptions,
};
use linked_hash_map::LinkedHashMap;
use std::{
    path::Path,
    process::{Command, Stdio},
};

use crate::{
    age_utils,
    api::WorkspaceApi,
    cli::{ConfigPart, WorkParams, WorkspacePersistence},
    constants,
    labels::{self, Labels, ROLE},
    model::{
        config::{ConfigSource, FileFormat, FinalCfg, RoozCfg},
        types::{AnyError, ContainerResult, RunSpec, WorkSpec, WorkspaceResult},
        volume::{RoozVolume, CACHE_ROLE, WORK_ROLE},
    },
    ssh,
};

impl<'a> WorkspaceApi<'a> {
    pub async fn create(&self, spec: &WorkSpec<'a>) -> Result<WorkspaceResult, AnyError> {
        let home_dir = format!("/home/{}", &spec.user);

        let mut volumes = vec![
            RoozVolume::home(spec.container_name.into(), &home_dir),
            RoozVolume::work(spec.container_name, constants::WORK_DIR),
        ];

        if let Some(caches) = &spec.caches {
            log::debug!("Processing caches");
            let cache_vols = caches
                .iter()
                .map(|p| RoozVolume::cache(p))
                .collect::<Vec<_>>();

            for c in caches {
                log::debug!("Cache: {}", c);
            }

            volumes.extend_from_slice(cache_vols.clone().as_slice());
        } else {
            log::debug!("No caches configured. Skipping");
        }

        let mut mounts = self
            .api
            .volume
            .ensure_mounts(&volumes, Some(&home_dir))
            .await?;

        mounts.push(ssh::mount(
            Path::new(&home_dir).join(".ssh").to_string_lossy().as_ref(),
        ));

        mounts.push(crate::age_utils::mount(
            Path::new(&home_dir).join(".age").to_string_lossy().as_ref(),
        ));

        let run_spec = RunSpec {
            reason: "work",
            image: &spec.image,
            uid: &spec.uid,
            user: &spec.user,
            work_dir: Some(&spec.container_working_dir),
            home_dir: &home_dir,
            container_name: &spec.container_name,
            workspace_key: &spec.workspace_key,
            mounts: Some(mounts),
            entrypoint: Some(vec!["cat"]),
            privileged: spec.privileged,
            force_recreate: spec.force_recreate,
            auto_remove: spec.ephemeral,
            labels: spec.labels.clone(),
            network: spec.network,
            env: spec.env_vars.clone(),
            ports: spec.ports.clone(),
            ..Default::default()
        };

        match self.api.container.create(run_spec).await? {
        ContainerResult::Created { .. } =>

            Ok(
                WorkspaceResult {
                    workspace_key: (&spec).workspace_key.to_string(),
                    working_dir: (&spec).container_working_dir.to_string(),
                    orig_uid: spec.uid.to_string(),
                    volumes: volumes.iter().map(|v|v.clone()).collect::<Vec<_>>() }),

        ContainerResult::AlreadyExists { .. } => {
            Err(format!("Container already exists. Did you mean: rooz enter {}? Otherwise, use --apply to reconfigure containers or --replace to recreate the whole workspace.", spec.workspace_key).into())
        }
    }
    }

    async fn remove_containers(&self, labels: &Labels, force: bool) -> Result<(), AnyError> {
        for cs in self.api.container.get_all(labels).await? {
            if let ContainerSummary { id: Some(id), .. } = cs {
                self.api.container.remove(&id, force).await?
            }
        }
        Ok(())
    }

    async fn remove_core(&self, labels: &Labels, force: bool) -> Result<(), AnyError> {
        self.remove_containers(labels, force).await?;
        let ls_vol_options = ListVolumesOptions {
            filters: labels.into(),
            ..Default::default()
        };

        if let Some(volumes) = self
            .api
            .client
            .list_volumes(Some(ls_vol_options))
            .await?
            .volumes
        {
            for v in volumes {
                match v {
                    Volume { ref name, .. }
                        if name == ssh::VOLUME_NAME || name == age_utils::VOLUME_NAME =>
                    {
                        continue;
                    }
                    Volume { labels, .. } => match labels.get(ROLE) {
                        Some(role) if role == CACHE_ROLE => continue,
                        _ => {}
                    },
                };
                self.api.volume.remove_volume(&v.name, force).await?
            }
        }

        let ls_network_options = ListNetworksOptions {
            filters: labels.into(),
        };
        for n in self
            .api
            .client
            .list_networks(Some(ls_network_options))
            .await?
        {
            if let Some(name) = n.name {
                let force_display = if force { " (force)" } else { "" };
                log::debug!("Remove network: {}{}", &name, &force_display);
                self.api.client.remove_network(&name).await?
            }
        }

        log::debug!("Remove success");
        Ok(())
    }

    pub async fn remove(&self, workspace_key: &str, force: bool) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), None);
        self.remove_core((&labels).into(), force).await?;
        Ok(())
    }

    pub async fn remove_containers_only(
        &self,
        workspace_key: &str,
        force: bool,
    ) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), None);
        self.remove_containers((&labels).into(), force).await?;
        Ok(())
    }

    pub async fn remove_all(&self, force: bool) -> Result<(), AnyError> {
        let labels = Labels::default();
        self.remove_core(&labels, force).await?;
        Ok(())
    }

    pub async fn start_workspace(&self, workspace_key: &str) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), None);
        for c in self.api.container.get_all(&labels).await? {
            self.api.container.start(&c.id.unwrap()).await?;
        }
        Ok(())
    }

    pub async fn stop(&self, workspace_key: &str) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), None);
        for c in self.api.container.get_all(&labels).await? {
            self.api.container.stop(&c.id.unwrap()).await?;
        }
        Ok(())
    }

    pub async fn stop_all(&self) -> Result<(), AnyError> {
        let labels = Labels::default();
        for c in self.api.container.get_all(&labels).await? {
            self.api.container.stop(&c.id.unwrap()).await?;
        }
        Ok(())
    }

    pub async fn show_config(&self, workspace_key: &str, part: ConfigPart) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), Some(WORK_ROLE));
        for c in self.api.container.get_all(&labels).await? {
            if let Some(labels) = c.labels {
                println!(
                    "{}",
                    labels[match part {
                        ConfigPart::OriginPath => labels::CONFIG_ORIGIN,
                        ConfigPart::OriginBody => labels::CONFIG_BODY,
                        ConfigPart::Runtime => labels::RUNTIME_CONFIG,
                    }]
                );
            }
        }
        Ok(())
    }

    pub fn encrypt(
        &self,
        identity: Identity,
        name: &str,
        secrets: LinkedHashMap<String, String>,
    ) -> Result<LinkedHashMap<String, String>, AnyError> {
        let encrypted = self.encrypt_value(identity, secrets[name].to_string())?;
        let mut new_secrets = secrets.clone();
        new_secrets.insert(name.to_string(), encrypted);
        Ok(new_secrets)
    }

    pub fn encrypt_value(
        &self,
        identity: Identity,
        clear_text: String,
    ) -> Result<String, AnyError> {
        age_utils::encrypt(clear_text, identity.to_public())
    }

    pub async fn decrypt(
        &self,
        secrets: Option<LinkedHashMap<String, String>>,
    ) -> Result<LinkedHashMap<String, String>, AnyError> {
        match secrets {
            Some(secrets) if secrets.len() > 0 => {
                log::debug!("Decrypting secrets");
                let identity = self.read_age_identity().await?;
                age_utils::decrypt(&identity, secrets)
            }
            Some(_) => Ok(LinkedHashMap::<String, String>::new()),
            None => Ok(LinkedHashMap::<String, String>::new()),
        }
    }

    pub async fn edit(&self, workspace_key: &str, spec: &WorkParams) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), Some(WORK_ROLE));
        for c in self.api.container.get_all(&labels).await? {
            if let Some(labels) = c.labels {
                let config_source = &labels[labels::CONFIG_ORIGIN];
                let format = FileFormat::from_path(config_source);
                let config =
                    RoozCfg::deserialize_config(&labels[labels::CONFIG_BODY], format)?.unwrap();
                let decrypted = self.decrypt(config.clone().secrets).await?;
                let decrypted_config = RoozCfg {
                    secrets: if decrypted.len() > 0 {
                        Some(decrypted.clone())
                    } else {
                        None
                    },
                    ..config
                };

                let decrypted_string = decrypted_config.to_string(format)?;
                let edited_string = edit::edit(decrypted_string.clone())?;

                //TODO: this check should be performed on the fully constructed config (to pick up changes in e.g. ROOZ_ env vars)
                if edited_string != decrypted_string {
                    let edited_config = RoozCfg::from_string(&edited_string, format)?;

                    match (&edited_config.vars, &edited_config.secrets) {

                        (Some(vars), Some(secrets)) => {
                            if let Some(duplicate_key) = vars.keys().find(|k| secrets.contains_key(&k.to_string())) {
                                panic!("The key: '{}' can be only defined in either vars or secrets." ,&duplicate_key.to_string())
                            }
                        },
                        _ => ()
                    };

                    let identity = self.read_age_identity().await?;

                    let mut encrypted_secrets = LinkedHashMap::<String, String>::new();
                    if let Some(edited_secrets) = &edited_config.clone().secrets {
                        for (k, v) in edited_secrets {
                            encrypted_secrets.insert(
                                k.to_string(),
                                self.encrypt_value(identity.clone(), v.to_string())?,
                            );
                        }
                    };
                    let encrypted_config = RoozCfg {
                        secrets: if encrypted_secrets.len() > 0 {
                            Some(encrypted_secrets)
                        } else {
                            None
                        },
                        ..edited_config
                    };

                    self.new(
                        spec,
                        Some(ConfigSource::Body {
                            value: encrypted_config,
                            origin: config_source.to_string(),
                            format,
                        }),
                        Some(WorkspacePersistence {
                            name: labels[labels::WORKSPACE_KEY].to_string(),
                            replace: false,
                            apply: true,
                        }),
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    pub async fn attach_vscode(&self, workspace_key: &str) -> Result<(), AnyError> {
        self.start_workspace(workspace_key).await?;

        let hex = format!(r#"{{"containerName":"{}"}}"#, workspace_key)
            .as_bytes()
            .iter()
            .map(|&b| format!("{:02x}", b))
            .collect::<Vec<String>>()
            .join("");
        let mut command = Command::new("code");
        command.arg("--folder-uri");
        command.arg(format!("vscode-remote://attached-container+{}/work", hex));
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        match command.spawn() {
            Ok(_) => Ok(()),
            Err(e) => Err(Box::new(e)),
        }
    }

    pub async fn enter(
        &self,
        workspace_key: &str,
        working_dir: Option<&str>,
        shell: Option<Vec<&str>>,
        container_id: Option<&str>,
        volumes: Vec<RoozVolume>,
        chown_uid: &str,
        root: bool,
        ephemeral: bool,
    ) -> Result<(), AnyError> {
        println!("{}", termion::clear::All);

        let enter_labels = Labels::new(Some(workspace_key), None)
            .with_container(container_id.or(Some(constants::DEFAULT_CONTAINER_NAME)));
        let summaries = self.api.container.get_all(&enter_labels).await?;

        let summary = match &summaries.as_slice() {
            &[container] => container,
            &[] => panic!("Container not found"),
            _ => panic!("Too many containers found"),
        };

        let mut shell_value = vec![constants::DEFAULT_SHELL.to_string()];

        if let Some(labels) = &summary.labels {
            if labels.contains_key(labels::RUNTIME_CONFIG) {
                shell_value = FinalCfg::from_string(labels[labels::RUNTIME_CONFIG].clone())?.shell;
            }
        }

        if let Some(shell) = shell {
            shell_value = shell.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        }

        let container_id = summary.id.as_deref().unwrap();

        self.start_workspace(workspace_key).await?;

        if !root {
            self.api.exec.ensure_user(container_id).await?;
            for v in &volumes {
                self.api
                    .exec
                    .chown(&container_id, chown_uid, &v.path)
                    .await?;
            }
        }

        self.api
            .exec
            .tty(
                "work",
                &container_id,
                true,
                working_dir,
                if root {
                    Some(constants::ROOT_USER)
                } else {
                    None
                },
                Some(shell_value.iter().map(|v| v.as_str()).collect::<Vec<_>>()),
            )
            .await?;

        if ephemeral {
            self.api.container.kill(&container_id).await?;
            for vol in volumes.iter().filter(|v| v.is_exclusive()) {
                self.api
                    .volume
                    .remove_volume(&vol.safe_volume_name(), true)
                    .await?;
            }
        }
        Ok(())
    }
}
