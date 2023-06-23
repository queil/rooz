use bollard::{
    network::ListNetworksOptions,
    service::{ContainerSummary, Volume},
    volume::ListVolumesOptions,
};

use crate::{
    backend::WorkspaceApi,
    constants,
    labels::Labels,
    ssh,
    types::{
        ContainerResult, RoozVolume, RoozVolumeRole, RoozVolumeSharing, RunSpec, WorkSpec,
        WorkspaceResult,
    },
};

impl<'a> WorkspaceApi<'a> {
    pub async fn create(
        &self,
        spec: &WorkSpec<'a>,
    ) -> Result<WorkspaceResult, Box<dyn std::error::Error + 'static>> {
        let home_dir = format!("/home/{}", &spec.user);
        let work_dir = format!("{}/work", &home_dir);

        let mut volumes = vec![
            RoozVolume {
                path: home_dir.clone(),
                sharing: RoozVolumeSharing::Exclusive {
                    key: spec.container_name.into(),
                },
                role: RoozVolumeRole::Home,
            },
            RoozVolume {
                path: work_dir.clone(),
                sharing: RoozVolumeSharing::Exclusive {
                    key: spec.container_name.into(),
                },
                role: RoozVolumeRole::Work,
            },
        ];

        if let Some(caches) = &spec.caches {
            log::debug!("Processing caches");
            let cache_vols = caches
                .iter()
                .map(|p| RoozVolume {
                    path: p.to_string(),
                    sharing: RoozVolumeSharing::Shared,
                    role: RoozVolumeRole::Cache,
                })
                .collect::<Vec<_>>();

            for c in caches {
                log::debug!("Cache: {}", c);
            }

            volumes.extend_from_slice(cache_vols.clone().as_slice());
        } else {
            log::debug!("No caches configured. Skipping");
        }

        let mut mounts = self.api.volume.ensure_mounts(&volumes, &home_dir).await?;

        if let Some(m) = &spec.git_vol_mount {
            mounts.push(m.clone());
        }

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
            ..Default::default()
        };

        match self.api.container.create(run_spec).await? {
        ContainerResult::Created { id } => Ok(WorkspaceResult { container_id: id, volumes: volumes.iter().map(|v|v.clone()).collect::<Vec<_>>() }),
        ContainerResult::AlreadyExists { .. } => {
            Err(format!("Container already exists. Did you mean: rooz enter {}? Otherwise, use --force to recreate.", spec.workspace_key).into())
        }
    }
    }

    async fn remove_core(
        &self,
        labels: &Labels,
        force: bool,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        for cs in self.api.container.get_all(labels).await? {
            if let ContainerSummary { id: Some(id), .. } = cs {
                self.api.container.remove(&id, force).await?
            }
        }

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
                    Volume { ref name, .. } if name == ssh::ROOZ_SSH_KEY_VOLUME_NAME => {
                        continue;
                    }
                    _ => {}
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

    pub async fn remove(
        &self,
        workspace_key: &str,
        force: bool,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let labels = Labels::new(Some(workspace_key), None);
        self.remove_core((&labels).into(), force).await?;
        Ok(())
    }

    pub async fn remove_all(
        &self,
        force: bool,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let labels = Labels::new(None, None);
        self.remove_core(&labels, force).await?;
        Ok(())
    }

    pub async fn start_workspace(
        &self,
        workspace_key: &str,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let labels = Labels::new(Some(workspace_key), None);
        for c in self.api.container.get_all(&labels).await? {
            self.api.container.start(&c.id.unwrap()).await?;
        }
        Ok(())
    }

    pub async fn stop(
        &self,
        workspace_key: &str,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let labels = Labels::new(Some(workspace_key), None);
        for c in self.api.container.get_all(&labels).await? {
            self.api.container.stop(&c.id.unwrap()).await?;
        }
        Ok(())
    }

    pub async fn stop_all(&self) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let labels = Labels::new(None, None);
        for c in self.api.container.get_all(&labels).await? {
            self.api.container.stop(&c.id.unwrap()).await?;
        }
        Ok(())
    }

    pub async fn enter(
        &self,
        workspace_key: &str,
        working_dir: Option<&str>,
        chown_dir: Option<&str>,
        shell: &str,
        container_id: Option<&str>,
        volumes: Vec<RoozVolume>,
        chown_uid: &str,
        root: bool,
        ephemeral: bool,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let container_id = container_id.unwrap_or(workspace_key);
        self.start_workspace(workspace_key).await?;

        if !root {
            self.api.exec.ensure_user(container_id).await?;
            if let Some(dir) = &chown_dir {
                self.api.exec.chown(&container_id, chown_uid, dir).await?;
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
                Some(vec![shell]),
            )
            .await?;

        if ephemeral {
            self.api.container.stop(&container_id).await?;
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
