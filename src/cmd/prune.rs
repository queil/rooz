use crate::{api::Api, model::types::AnyError, util::labels::Labels};
use bollard::{
    query_parameters::{ListContainersOptions, ListVolumesOptions, RemoveVolumeOptions},
    service::ContainerSummary,
};
use bollard_stubs::query_parameters::ListImagesOptions;

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
                log::debug!("Remove container: {}", &id);
                self.container.remove(&id, force).await?
            }
        }

        let ls_images_options = ListImagesOptions {
            all: true,
            filters: Some(filters.clone().into()),
            ..Default::default()
        };

        for img in self.client.list_images(Some(ls_images_options)).await? {
            log::debug!("Remove image: {}", &img.id);
            self.image.remove_local(&img.id, force).await?
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
                log::debug!("Remove volume: {}", &v.name);
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
