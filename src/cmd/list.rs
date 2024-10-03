use crate::{
    model::types::AnyError,
    util::labels::{self, Labels, CONFIG_ORIGIN},
};

use bollard::{container::ListContainersOptions, service::ContainerSummary, Docker};

use tabled::{settings::Style, Table, Tabled};

#[derive(Debug, Tabled)]
struct WorkspaceView {
    #[tabled(rename = "WORKSPACE")]
    name: String,
    #[tabled(rename = "RUNNING", format("{}", if self.running {"true"} else {""}))]
    running: bool,
    #[tabled(rename = "CONFIG")]
    origin: String,
}

pub async fn list(docker: &Docker) -> Result<(), AnyError> {
    let labels = Labels::new(None, Some(labels::ROLE_WORK));
    let list_options = ListContainersOptions {
        filters: (&labels).into(),
        all: true,
        ..Default::default()
    };

    let container_summary = docker.list_containers(Some(list_options)).await?;

    let mut views = Vec::<WorkspaceView>::new();

    for c in container_summary {
        if let ContainerSummary {
            labels: Some(labels),
            state: Some(state),
            ..
        } = c
        {
            let is_running = match state.as_str() {
                "running" => true,
                _ => false,
            };
            views.push(WorkspaceView {
                name: c.names.unwrap().join(", ")[1..].to_string(),
                running: is_running,
                origin: labels
                    .get(CONFIG_ORIGIN)
                    .unwrap_or(&"cli".to_string())
                    .to_string(),
            });
        }
    }

    views.sort_by(|a, b| a.name.cmp(&b.name));

    let table = Table::new(views).with(Style::blank()).to_string();

    println!("{}", table);
    Ok(())
}
