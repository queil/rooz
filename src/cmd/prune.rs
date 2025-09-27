use bollard::{
    query_parameters::{ListContainersOptions, ListVolumesOptions, RemoveVolumeOptions},
    service::ContainerSummary,
};

use crate::{api::Api, model::types::AnyError, util::labels::Labels};

impl<'a> Api<'a> {
    async fn prune(&self, filters: Labels, force: bool) -> Result<(), AnyError> {
        let ls_container_options = ListContainersOptions {
            all: true,
            filters: Some(filters.clone().into()),
            ..Default::default()
        };
        for cs in self
            .client
            .list_containers(Some(ls_container_options))
            .await?
        {
            if let ContainerSummary { id: Some(id), .. } = cs {
                log::debug!("Force remove container: {}", &id);
                self.container.remove(&id, force).await?
            }
        }

        let ls_vol_options = ListVolumesOptions {
            filters: Some(filters.into()),
            ..Default::default()
        };

        if let Some(volumes) = self
            .client
            .list_volumes(Some(ls_vol_options))
            .await?
            .volumes
        {
            let rm_vol_options = RemoveVolumeOptions {
                force,
                ..Default::default()
            };

            for v in volumes {
                log::debug!("Force remove volume: {}", &v.name);
                self.client
                    .remove_volume(&v.name, Some(rm_vol_options.clone()))
                    .await?
            }
        }
        log::debug!("Prune success");
        Ok(())
    }

    pub async fn prune_system(&self) -> Result<(), AnyError> {
        self.prune(Labels::default(), true).await
    }
}
