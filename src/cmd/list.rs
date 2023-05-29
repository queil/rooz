use crate::labels;
use std::collections::HashMap;

use bollard::{container::ListContainersOptions, service::ContainerSummary, Docker};

pub async fn list(docker: &Docker) -> Result<(), Box<dyn std::error::Error + 'static>> {
    let is_workspace = labels::is_workspace();
    let list_options = ListContainersOptions {
        filters: HashMap::from([("label", vec![is_workspace.as_ref()])]),
        all: true,
        ..Default::default()
    };

    let container_summary = docker.list_containers(Some(list_options)).await?;
    println!("WORKSPACE");

    for c in container_summary {
        if let ContainerSummary {
            labels: Some(labels),
            state: Some(state),
            ..
        } = c
        {
            let state_icon = match state.as_str() {
                "running" => "✱",
                _ => " ",
            };
            println!("{} {}", labels[labels::WORKSPACE_KEY], state_icon);
        }
    }
    Ok(())
}
