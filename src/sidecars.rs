use std::collections::HashMap;

use bollard::network::CreateNetworkOptions;

use crate::{
    backend::WorkspaceApi,
    constants,
    labels::{self, Labels},
    types::{
        AnyError, RoozCfg, RoozSidecar, RoozVolume, RoozVolumeRole, RoozVolumeSharing, RunSpec,
    },
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

            match self.api.client.create_network(network_options).await {
                Ok(_) => (),
                Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 409,
                    message,
                }) => {
                    log::debug!("Could not create network: {}", message);
                }
                e => panic!("{:?}", e),
            };
            Some(workspace_key.as_ref())
        } else {
            None
        };

        if let Some(sidecars) = sidecars {
            for (name, s) in sidecars {
                log::debug!("Process sidecar: {}", name);
                self.api.image.ensure(&s.image, pull_image).await?;
                let container_name = format!("{}-{}", workspace_key, name);
                let labels = labels_sidecar.clone().with_container(Some(name));
                let mut ports = HashMap::<String, String>::new();
                RoozCfg::parse_ports(&mut ports, s.ports.clone());

                self.api
                    .container
                    .create(RunSpec {
                        container_name: &container_name,
                        uid: constants::ROOT_UID,
                        image: &s.image,
                        force_recreate: force,
                        workspace_key: &workspace_key,
                        labels: (&labels).into(),
                        env: s.env.clone(),
                        network,
                        network_aliases: Some(vec![name.into()]),
                        command: s
                            .command
                            .as_ref()
                            .map(|x| x.iter().map(|z| z.as_ref()).collect()),
                        mounts: match s.mounts.as_ref().map(|paths| {
                            paths
                                .iter()
                                .map(|path| RoozVolume {
                                    path: path.into(),
                                    role: RoozVolumeRole::Data,
                                    sharing: RoozVolumeSharing::Exclusive {
                                        key: container_name.to_string(),
                                    },
                                })
                                .collect::<Vec<_>>()
                        }) {
                            Some(ms) => Some(self.api.volume.ensure_mounts(&ms, None).await?),
                            None => None,
                        },
                        ports: Some(ports),
                        ..Default::default()
                    })
                    .await?;
            }
        }
        Ok(network.map(|n| n.to_string()))
    }
}
