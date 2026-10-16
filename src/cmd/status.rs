use bollard::query_parameters::{ListContainersOptions, ListVolumesOptions};
use serde::Serialize;
use tabled::{Table, Tabled, settings::Style};

use crate::{
    api::Api,
    cmd::list::{human_age, now_secs},
    model::types::AnyError,
    util::labels::{CONFIG_ORIGIN, Labels, ROLE, WORKSPACE_CONFIG_ROLE},
};

#[derive(Debug, Serialize, Tabled)]
struct ContainerRow {
    #[tabled(rename = "CONTAINER")]
    name: String,
    #[tabled(rename = "ROLE")]
    role: String,
    #[tabled(rename = "STATE")]
    state: String,
    #[tabled(rename = "IMAGE")]
    image: String,
    #[tabled(skip)]
    created: Option<i64>,
    #[serde(skip)]
    #[tabled(rename = "AGE")]
    age: String,
}

#[derive(Debug, Serialize, Tabled)]
struct VolumeRow {
    #[tabled(rename = "VOLUME")]
    name: String,
    #[tabled(rename = "ROLE")]
    role: String,
}

#[derive(Debug, Serialize)]
struct Status {
    name: String,
    origin: String,
    created: Option<String>,
    containers: Vec<ContainerRow>,
    volumes: Vec<VolumeRow>,
}

impl<'a> Api<'a> {
    pub async fn status(&self, workspace_key: &str, json: bool) -> Result<(), AnyError> {
        let ws_labels = Labels::from(&[Labels::workspace(workspace_key)]);

        let config_volumes = self
            .client
            .list_volumes(Some(ListVolumesOptions {
                filters: Some(Labels::workspace_config(workspace_key).into()),
                ..Default::default()
            }))
            .await?
            .volumes
            .unwrap_or_default();
        let config_volume = config_volumes
            .iter()
            .find(|v| v.labels.get(ROLE).map(String::as_str) == Some(WORKSPACE_CONFIG_ROLE))
            .ok_or(format!(
                "Workspace not found: {}. Check 'rooz list'.",
                workspace_key
            ))?;

        let now = now_secs();
        let mut containers = self
            .client
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters: Some(ws_labels.clone().into()),
                ..Default::default()
            }))
            .await?
            .into_iter()
            .map(|c| ContainerRow {
                name: c
                    .names
                    .as_ref()
                    .and_then(|n| n.first())
                    .map(|n| n.trim_start_matches('/').to_string())
                    .unwrap_or_default(),
                role: c
                    .labels
                    .as_ref()
                    .and_then(|l| l.get(ROLE).cloned())
                    .unwrap_or_default(),
                state: c.state.map(|s| s.to_string()).unwrap_or_default(),
                image: c.image.unwrap_or_default(),
                age: human_age(c.created, now),
                created: c.created,
            })
            .collect::<Vec<_>>();
        containers.sort_by(|a, b| a.name.cmp(&b.name));

        let mut volumes = self
            .client
            .list_volumes(Some(ListVolumesOptions {
                filters: Some(ws_labels.into()),
                ..Default::default()
            }))
            .await?
            .volumes
            .unwrap_or_default()
            .into_iter()
            .map(|v| VolumeRow {
                role: v.labels.get(ROLE).cloned().unwrap_or_default(),
                name: v.name,
            })
            .collect::<Vec<_>>();
        volumes.sort_by(|a, b| a.name.cmp(&b.name));

        let status = Status {
            name: workspace_key.to_string(),
            origin: config_volume
                .labels
                .get(CONFIG_ORIGIN)
                .cloned()
                .unwrap_or_else(|| "cli".to_string()),
            created: config_volume.created_at.clone(),
            containers,
            volumes,
        };

        if json {
            println!("{}", serde_json::to_string_pretty(&status)?);
            return Ok(());
        }

        println!("Workspace: {}", status.name);
        println!("Config:    {}", status.origin);
        if let Some(created) = &status.created {
            println!("Created:   {}", created);
        }
        println!("\n{}", Table::new(&status.containers).with(Style::blank()));
        println!("\n{}", Table::new(&status.volumes).with(Style::blank()));
        Ok(())
    }
}
