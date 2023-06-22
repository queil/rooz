use std::collections::HashMap;

use bollard::{
    container::ListContainersOptions,
    service::ContainerSummary,
    volume::{ListVolumesOptions, RemoveVolumeOptions},
};

use crate::{labels::Labels, backend::{Api}};

impl<'a> Api<'a> {
    async fn prune(
        &self,
        filters: HashMap<String, Vec<String>>,
        force: bool,
    ) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let ls_container_options = ListContainersOptions {
            all: true,
            filters: filters.clone(),
            ..Default::default()
        };
        for cs in self.client.list_containers(Some(ls_container_options)).await? {
            if let ContainerSummary { id: Some(id), .. } = cs {
                log::debug!("Force remove container: {}", &id);
                self.container.remove(&id, force).await?
            }
        }

        let ls_vol_options = ListVolumesOptions {
            filters: filters.clone(),
            ..Default::default()
        };

        if let Some(volumes) = self.client.list_volumes(Some(ls_vol_options)).await?.volumes {
            let rm_vol_options = RemoveVolumeOptions {
                force,
                ..Default::default()
            };

            for v in volumes {
                log::debug!("Force remove volume: {}", &v.name);
                self.client.remove_volume(&v.name, Some(rm_vol_options)).await?
            }
        }
        log::debug!("Prune success");
        Ok(())
    }

    pub async fn prune_system(&self) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let labels = Labels::new(None, None);
        self.prune((&labels).into(), true).await
    }
}
