use bollard::{
    query_parameters::{ListNetworksOptions, ListVolumesOptions},
    service::{ContainerSummary, Volume},
};

use crate::{
    api::WorkspaceApi,
    model::types::AnyError,
    util::labels::{Labels, ROLE, WORKSPACE_CONFIG_ROLE},
};

impl<'a> WorkspaceApi<'a> {
    async fn remove_containers(&self, labels: &Labels, force: bool) -> Result<(), AnyError> {
        for cs in self.api.container.get_all(labels).await? {
            if let ContainerSummary { id: Some(id), .. } = cs {
                self.api.container.remove(&id, force).await?
            }
        }
        Ok(())
    }

    async fn remove_core<F>(&self, labels: &Labels, filter: F, force: bool) -> Result<(), AnyError>
    where
        F: FnMut(&&Volume) -> bool,
    {
        self.remove_containers(labels, force).await?;
        let ls_vol_options = ListVolumesOptions {
            filters: Some(labels.clone().into()),
            ..Default::default()
        };

        if let Some(volumes) = self
            .api
            .client
            .list_volumes(Some(ls_vol_options))
            .await?
            .volumes
        {
            for v in volumes.iter().filter(filter) {
                self.api.volume.remove_volume(&v.name, force).await?
            }
        }

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
        keep_config: bool,
        force: bool,
    ) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);
        self.remove_core(
            (&labels).into(),
            |v| match v.labels.get(ROLE) {
                Some(r) if r == WORKSPACE_CONFIG_ROLE => !keep_config,
                _ => true,
            },
            force,
        )
        .await?;
        Ok(())
    }

    pub async fn remove_containers_only(
        &self,
        workspace_key: &str,
        force: bool,
    ) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);
        self.remove_containers((&labels).into(), force).await?;
        Ok(())
    }

    pub async fn remove_all(&self, force: bool) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::any_workspace()]);
        self.remove_core(&labels, |_| true, force).await?;
        Ok(())
    }
}
