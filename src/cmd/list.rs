use crate::{
    api::Api,
    model::types::AnyError,
    util::labels::{Labels, CONFIG_ORIGIN, ROLE_WORK},
};

use bollard::{query_parameters::ListContainersOptions, service::ContainerSummary};

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

impl<'a> Api<'a> {
    pub async fn list(&self) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::role(ROLE_WORK)]);
        let list_options = ListContainersOptions {
            all: true,
            filters: Some(labels.into()),
            ..Default::default()
        };

        let container_summary = self.client.list_containers(Some(list_options)).await?;

        let mut views = Vec::<WorkspaceView>::new();

        for c in container_summary {
            if let ContainerSummary {
                labels: Some(labels),
                state: Some(state),
                ..
            } = c
            {
                let is_running = match state {
                    bollard::models::ContainerSummaryStateEnum::RUNNING => true,
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
}
