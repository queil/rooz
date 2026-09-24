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

        for v in volumes.volumes.unwrap_or_default() {
            // labels come from the engine and anyone with engine access can write them:
            // a volume carrying the role but no workspace is somebody else's, not a
            // reason to abort the listing
            let Some(workspace_key) = v.labels.get(WORKSPACE_KEY) else {
                log::debug!("Skipping volume without a workspace label: {}", v.name);
                continue;
            };
            let is_running = containers.iter().any(|c| {
                c.labels
                    .as_ref()
                    .and_then(|l| l.get(WORKSPACE_KEY))
                    .is_some_and(|key| key == workspace_key)
            });
            views.push(WorkspaceView {
                name: workspace_key.to_string(),
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
