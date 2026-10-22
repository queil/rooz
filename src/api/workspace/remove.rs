use crate::{
    api::{WorkspaceApi, checkpoint},
    model::types::AnyError,
    util::labels::{Labels, ROLE, WORKSPACE_CONFIG_ROLE},
};
use bollard::{
    query_parameters::{ListNetworksOptions, ListVolumesOptions},
    service::{ContainerSummary, Volume},
};

impl<'a> WorkspaceApi<'a> {
    async fn remove_containers(
        &self,
        labels: &Labels,
        force: bool,
        keep_images: &[&str],
        keep_checkpoints: bool,
    ) -> Result<(), AnyError> {
        let containers = self.api.container.get_all(labels).await?;
        let container_api = self.api.container;

        let futures = containers.into_iter().filter_map(|cs| {
            if let ContainerSummary { id: Some(id), .. } = cs {
                Some(async move { container_api.remove(&id, force).await })
            } else {
                None
            }
        });

        futures::future::try_join_all(futures).await?;
        self.remove_images(labels, force, keep_images, keep_checkpoints)
            .await?;
        Ok(())
    }

    async fn remove_images(
        &self,
        labels: &Labels,
        force: bool,
        keep: &[&str],
        keep_checkpoints: bool,
    ) -> Result<(), AnyError> {
        let (checkpoints, images): (Vec<_>, Vec<_>) = self
            .api
            .image
            .get_all(labels)
            .await?
            .into_iter()
            .partition(checkpoint::is_checkpoint);
        let image_api = self.api.image;
        let bare = |id: &str| id.trim_start_matches("sha256:").to_string();
        let keep = keep.iter().map(|id| bare(id)).collect::<Vec<_>>();

        let futures = images
            .into_iter()
            .filter(|img| !keep.contains(&bare(&img.id)))
            .map(|img| async move { image_api.remove_local(&img.id, force).await });

        futures::future::try_join_all(futures).await?;
        // after the images built from them
        if !keep_checkpoints {
            checkpoint::remove(self.api.client, &checkpoints).await;
        }
        Ok(())
    }

    async fn remove_core<F>(&self, labels: &Labels, filter: F, force: bool) -> Result<(), AnyError>
    where
        F: FnMut(&&Volume) -> bool,
    {
        self.remove_containers(labels, force, &[], false).await?;
        let ls_vol_options = ListVolumesOptions {
            filters: Some(labels.clone().into()),
        };

        if let Some(volumes) = self
            .api
            .client
            .list_volumes(Some(ls_vol_options))
            .await?
            .volumes
        {
            let volume_api = self.api.volume;
            let futures = volumes
                .iter()
                .filter(filter)
                .map(|v| async move { volume_api.remove_volume(&v.name, force).await });
            futures::future::try_join_all(futures).await?;
        }

        self.remove_networks(labels).await?;

        log::debug!("Remove success");
        Ok(())
    }

    async fn remove_networks(&self, labels: &Labels) -> Result<(), AnyError> {
        let ls_network_options = ListNetworksOptions {
            filters: Some(labels.clone().into()),
        };
        for n in self
            .api
            .client
            .list_networks(Some(ls_network_options))
            .await?
        {
            if let Some(name) = n.name {
                log::debug!("Remove network: {}", name);
                self.api.client.remove_network(&name).await?
            }
        }
        Ok(())
    }

    pub async fn remove(
        &self,
        workspace_key: &str,
        keep_config: bool,
        force: bool,
    ) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);
        self.remove_core(
            &labels,
            |v| match v.labels.get(ROLE) {
                Some(r) if r == WORKSPACE_CONFIG_ROLE => !keep_config,
                _ => true,
            },
            force,
        )
        .await?;
        Ok(())
    }

    // `keep_images` are images still to be reused, e.g. the sidecar images 'update' carries over;
    // `keep_checkpoints` keeps the install progress of a workspace whose creation did not finish
    pub async fn remove_containers_only(
        &self,
        workspace_key: &str,
        force: bool,
        keep_images: &[&str],
        keep_checkpoints: bool,
    ) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);
        self.remove_containers(&labels, force, keep_images, keep_checkpoints)
            .await?;
        // recreated with the containers; this also retires networks named by an older rooz
        self.remove_networks(&labels).await?;
        Ok(())
    }

    pub async fn remove_all(&self, force: bool) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::any_workspace()]);
        self.remove_core(&labels, |_| true, force).await?;
        Ok(())
    }
}
