use std::collections::HashMap;
use crate::labels;

use bollard::{Docker, volume::ListVolumesOptions, service::VolumeListResponse};

pub async fn list(
    docker: &Docker) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let is_workspace = labels::is_workspace();
        let list_options = ListVolumesOptions {
            filters: HashMap::from([("label", vec![is_workspace.as_ref()])]),
        };
        let VolumeListResponse{ volumes,..} = docker.list_volumes(Some(list_options)).await?;
        println!("WORKSPACE");
        if let Some(volumes) = volumes {
            for v in volumes {
                println!("{}", v.labels[labels::GROUP_KEY]);
            }
        };
        Ok(())
}
