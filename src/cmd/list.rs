use crate::{
    api::Api,
    model::types::AnyError,
    util::labels::{CONFIG_ORIGIN, Labels, WORK_ROLE, WORKSPACE_CONFIG_ROLE, WORKSPACE_KEY},
};

use bollard::query_parameters::{ListContainersOptions, ListVolumesOptions};
use serde::Serialize;
use tabled::{Table, Tabled, settings::Style};

#[derive(Debug, Tabled)]
struct WorkspaceView {
    #[tabled(rename = "WORKSPACE")]
    name: String,
    #[tabled(rename = "STATUS")]
    status: String,
    #[tabled(rename = "IMAGE")]
    image: String,
    #[tabled(rename = "AGE")]
    age: String,
    #[tabled(rename = "CONFIG")]
    origin: String,
}

#[derive(Debug, Serialize)]
struct WorkspaceJson {
    name: String,
    status: String,
    image: String,
    created: Option<i64>,
    origin: String,
}

pub fn now_secs() -> i64 {
    jiff::Timestamp::now().as_second()
}

pub fn human_age(created: Option<i64>, now: i64) -> String {
    created
        .map(|c| {
            timeago::Formatter::new()
                .num_items(1)
                .ago("")
                .convert(std::time::Duration::from_secs((now - c).max(0) as u64))
        })
        .unwrap_or_default()
}

impl<'a> Api<'a> {
    pub async fn list(&self, json: bool) -> Result<(), AnyError> {
        let volume_labels = Labels::from(&[Labels::role(WORKSPACE_CONFIG_ROLE)]);
        let list_options = ListVolumesOptions {
            filters: Some(volume_labels.into()),
            ..Default::default()
        };

        let volumes = self.client.list_volumes(Some(list_options)).await?;

        let container_labels = Labels::from(&[Labels::role(WORK_ROLE)]);

        let options = Some(ListContainersOptions {
            all: true,
            filters: Some(container_labels.into()),
            ..Default::default()
        });

        let containers = self.client.list_containers(options).await?;

        let mut rows = Vec::<WorkspaceJson>::new();

        for v in volumes.volumes.unwrap_or_default() {
            // labels come from the engine and anyone with engine access can write them:
            // a volume carrying the role but no workspace is somebody else's, not a
            // reason to abort the listing
            let Some(workspace_key) = v.labels.get(WORKSPACE_KEY) else {
                log::debug!("Skipping volume without a workspace label: {}", v.name);
                continue;
            };
            let container = containers.iter().find(|c| {
                c.labels
                    .as_ref()
                    .and_then(|l| l.get(WORKSPACE_KEY))
                    .is_some_and(|key| key == workspace_key)
            });
            rows.push(WorkspaceJson {
                name: workspace_key.to_string(),
                status: container
                    .and_then(|c| c.state.as_ref())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "no container".to_string()),
                image: container.and_then(|c| c.image.clone()).unwrap_or_default(),
                created: container.and_then(|c| c.created),
                origin: v
                    .labels
                    .get(CONFIG_ORIGIN)
                    .cloned()
                    .unwrap_or_else(|| "cli".to_string()),
            });
        }

        rows.sort_by(|a, b| a.name.cmp(&b.name));

        if json {
            println!("{}", serde_json::to_string_pretty(&rows)?);
            return Ok(());
        }

        let now = now_secs();
        let views = rows.into_iter().map(|r| WorkspaceView {
            age: human_age(r.created, now),
            name: r.name,
            status: r.status,
            image: r.image,
            origin: r.origin,
        });

        println!("{}", Table::new(views).with(Style::blank()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::human_age;

    #[test]
    fn age_is_the_largest_unit_without_a_suffix() {
        assert_eq!(human_age(Some(0), 600), "10 minutes");
        assert_eq!(human_age(Some(0), 7200), "2 hours");
        assert_eq!(human_age(Some(0), 3 * 86400 + 5), "3 days");
        assert_eq!(human_age(None, 10), "");
        assert_eq!(human_age(Some(50), 10), human_age(Some(10), 10));
    }
}
