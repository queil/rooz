use crate::{
    api::Api,
    model::types::AnyError,
    util::labels::{CONFIG_ORIGIN, Labels, WORK_ROLE, WORKSPACE_CONFIG_ROLE, WORKSPACE_KEY},
};

use bollard::query_parameters::{ListContainersOptions, ListVolumesOptions};

use tabled::{Table, Tabled, settings::Style};

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
        let volume_labels = Labels::from(&[Labels::role(WORKSPACE_CONFIG_ROLE)]);
        let list_options = ListVolumesOptions {
            filters: Some(volume_labels.into()),
            ..Default::default()
        };

        let volumes = self.client.list_volumes(Some(list_options)).await?;

        let container_labels = Labels::from(&[Labels::role(WORK_ROLE)]);

        let options = Some(ListContainersOptions {
            all: false,
            filters: Some(container_labels.into()),
            ..Default::default()
        });

        let containers = self.client.list_containers(options).await?;

        let mut views = Vec::<WorkspaceView>::new();

        for v in volumes.volumes.unwrap() {
            let workspace_key = &v.labels[WORKSPACE_KEY];
            let is_running = containers
                .iter()
                .any(|c| (&c.labels).clone().unwrap_or_default()[WORKSPACE_KEY] == *workspace_key);
            views.push(WorkspaceView {
                name: (&v.labels[WORKSPACE_KEY]).to_string(),
                running: is_running,
                origin: (&v.labels.get(CONFIG_ORIGIN).unwrap_or(&"cli".to_string())).to_string(),
            });
        }

        views.sort_by(|a, b| a.name.cmp(&b.name));

        let table = Table::new(views).with(Style::blank()).to_string();

        println!("{}", table);
        Ok(())
    }
}
