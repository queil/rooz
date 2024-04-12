use std::path::Path;

use bollard::{
    network::ListNetworksOptions,
    service::{ContainerSummary, Volume},
    volume::ListVolumesOptions,
};

use crate::{
    age_utils,
    api::WorkspaceApi,
    constants,
    labels::{self, Labels, ROLE},
    model::{
        config::FinalCfg,
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

        let mut run_spec = RunSpec {
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

        log::debug!("Checking if env vars need decryption");
        if let Some(vars) = age_utils::needs_decryption(run_spec.env.clone()) {
            log::debug!("Decrypting vars");
            let identity = self.read_age_identity().await?;
            let decrypted_kv = age_utils::decrypt(&identity, vars)?;
            run_spec = RunSpec {
                env: Some(decrypted_kv),
                ..run_spec
            }
        } else {
            log::debug!("No encrypted vars found");
        }

        match self.api.container.create(run_spec).await? {
        ContainerResult::Created { id } =>

            Ok(
                WorkspaceResult {
                    workspace_key: (&spec).workspace_key.to_string(),
                    working_dir: (&spec).container_working_dir.to_string(),
                    home_dir,
                    orig_uid: spec.uid.to_string(),
                    container_id: id,
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
        let labels = Labels::new(None, None);
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
        let labels = Labels::new(None, None);
        for c in self.api.container.get_all(&labels).await? {
            self.api.container.stop(&c.id.unwrap()).await?;
        }
        Ok(())
    }

    pub async fn show_config(&self, workspace_key: &str) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), Some(WORK_ROLE));
        for c in self.api.container.get_all(&labels).await? {
            if let Some(labels) = c.labels {
                println!("{}", labels[labels::CONFIG]);
            }
        }
        Ok(())
    }

    pub async fn enter(
        &self,
        workspace_key: &str,
        working_dir: Option<&str>,
        shell: Option<&str>,
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

        let mut shell_value = constants::DEFAULT_SHELL.to_string();

        if let Some(labels) = &summary.labels {
            if labels.contains_key(labels::CONFIG) {
                shell_value = FinalCfg::from_string(labels[labels::CONFIG].clone())?.shell;
            }
        }

        if let Some(shell) = shell {
            shell_value = shell.into();
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
                Some(vec![&shell_value]),
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
