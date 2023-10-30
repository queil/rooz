use std::collections::HashMap;

use bollard::network::CreateNetworkOptions;

use crate::{
    backend::WorkspaceApi,
    constants,
    labels::{self, Labels},
    types::{AnyError, RoozSidecar, RunSpec},
};

impl<'a> WorkspaceApi<'a> {
    pub async fn ensure_sidecars(
        &self,
        sidecars: &Option<HashMap<String, RoozSidecar>>,
        labels: &Labels,
        workspace_key: &str,
        force: bool,
        pull_image: bool,
    ) -> Result<Option<String>, AnyError> {
        let labels_sidecar = Labels::new(Some(workspace_key), Some(labels::ROLE_SIDECAR));

        let network = if let Some(_) = sidecars {
            let network_options = CreateNetworkOptions::<&str> {
                name: &workspace_key,
                check_duplicate: true,
                labels: labels.into(),

                ..Default::default()
            };

            self.api.client.create_network(network_options).await?;
            Some(workspace_key.as_ref())
        } else {
            None
        };

        if let Some(sidecars) = sidecars {
            for (name, s) in sidecars {
                log::debug!("Process sidecar: {}", name);
                self.api.image.ensure(&s.image, pull_image).await?;
                let container_name = format!("{}-{}", workspace_key, name);
                self.api
                    .container
                    .create(RunSpec {
                        container_name: &container_name,
                        uid: constants::ROOT_UID,
                        image: &s.image,
                        force_recreate: force,
                        workspace_key: &workspace_key,
                        labels: (&labels_sidecar).into(),
                        env: s.env.clone(),
                        network,
                        network_aliases: Some(vec![name.into()]),
                        ..Default::default()
                    })
                    .await?;
            }
        }
        Ok(network.map(|n| n.to_string()))
    }
}
