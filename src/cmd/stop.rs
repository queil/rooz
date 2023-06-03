use std::collections::HashMap;

use bollard::{container::ListContainersOptions, service::ContainerSummary, Docker};

use crate::container;

pub async fn stop(
    docker: &Docker,
    filters: HashMap<String, Vec<String>>,
) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let ls_container_options = ListContainersOptions {
        filters: filters.clone(),
        ..Default::default()
    };
    for cs in docker.list_containers(Some(ls_container_options)).await? {
        if let ContainerSummary { id: Some(id), .. } = cs {
            log::debug!("Stop container: {}", &id);
            container::stop(&docker, &id).await?
        }
    }
    Ok(())
}
