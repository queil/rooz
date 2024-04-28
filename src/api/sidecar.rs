use std::collections::HashMap;

use bollard::network::CreateNetworkOptions;

use crate::{
    api::WorkspaceApi,
    constants,
    labels::{self, Labels},
    model::{
        config::{RoozCfg, RoozSidecar},
        types::{AnyError, RunSpec},
        volume::RoozVolume,
    },
};

impl<'a> WorkspaceApi<'a> {
    pub async fn ensure_sidecars(
        &self,
        sidecars: &HashMap<String, RoozSidecar>,
        workspace_key: &str,
        force: bool,
        pull_image: bool,
        work_dir: &str,
    ) -> Result<Option<String>, AnyError> {
        let labels = &Labels::new(Some(workspace_key), None);

        let network = if !sidecars.is_empty() {
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

        for (name, s) in sidecars {
            log::debug!("Process sidecar: {}", name);
            self.api.image.ensure(&s.image, pull_image).await?;
            let container_name = format!("{}-{}", workspace_key, name);
            let labels = labels
                .clone()
                .with_container(Some(&name))
                .with_role(Some(labels::ROLE_SIDECAR));
            let mut ports = HashMap::<String, Option<String>>::new();
            RoozCfg::parse_ports(&mut ports, s.ports.clone());

            let mut mounts = Vec::<RoozVolume>::new();

            let auto_mounts = s.mounts.as_ref().map(|paths| {
                paths
                    .iter()
                    .map(|path| RoozVolume::sidecar_data(workspace_key, path))
                    .collect::<Vec<_>>()
            });

            if let Some(v) = auto_mounts {
                mounts.extend_from_slice(&v.as_slice());
            }

            let work_mount = if let Some(true) = s.mount_work {
                Some(vec![RoozVolume::work(workspace_key, work_dir)])
            } else {
                None
            };

            if let Some(v) = work_mount {
                mounts.extend_from_slice(&v.as_slice());
            }

            self.api
                .container
                .create(RunSpec {
                    container_name: &container_name,
                    uid: &s.user.as_deref().unwrap_or(&constants::ROOT_UID),
                    image: &s.image,
                    force_recreate: force,
                    workspace_key: &workspace_key,
                    labels: (&labels).into(),
                    env: s.env.clone().map(|x| {
                        x.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<HashMap<_, _>>()
                    }),
                    network,
                    network_aliases: Some(vec![name.into()]),
                    command: s
                        .command
                        .as_ref()
                        .map(|x| x.iter().map(|z| z.as_ref()).collect()),
                    mounts: Some(self.api.volume.ensure_mounts(&mounts, None).await?),
                    ports: Some(ports),
                    work_dir: Some(s.work_dir.as_deref().unwrap_or(work_dir)),
                    ..Default::default()
                })
                .await?;
        }

        Ok(network.map(|n| n.to_string()))
    }
}
