use std::collections::HashMap;
use crate::labels;

use bollard::{Docker, service::{ContainerSummary}, container::ListContainersOptions};

pub async fn list(
    docker: &Docker) -> Result<(), Box<dyn std::error::Error + 'static>> {
        let is_workspace = labels::is_workspace();
        let list_options = ListContainersOptions {
            filters: HashMap::from([("label", vec![is_workspace.as_ref()])]),
            all: true,
            ..Default::default()
        };

        let container_summary = docker.list_containers(Some(list_options)).await?;
        println!("WORKSPACE");
        
            for c in container_summary {
                
                if let ContainerSummary {labels: Some(labels), ..} = c {
                    println!("{}", labels[labels::GROUP_KEY]);
                }
            }
        Ok(())
}
