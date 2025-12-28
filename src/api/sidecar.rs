use std::collections::HashMap;

use bollard::models::NetworkCreateRequest;

use crate::{
    api::WorkspaceApi,
    config::config::{RoozCfg, RoozSidecar, SidecarMount},
    constants,
    model::{
        types::{AnyError, RunMode, RunSpec},
        volume::VolumeBackedPath,
    },
    util::labels::{self, Labels},
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
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);

        let network = if !sidecars.is_empty() {
            let network_options = NetworkCreateRequest {
                name: workspace_key.into(),
                labels: Some(labels.clone().into()),
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
            let container_name = format!("{}-{}", workspace_key, name);
            let mut labels = labels.clone();
            labels.extend(&[Labels::container(&name), Labels::role(labels::SIDECAR_ROLE)]);
            let mut ports = HashMap::<String, Option<String>>::new();
            RoozCfg::parse_ports(&mut ports, s.ports.clone());

            let mut mounts = Vec::<VolumeBackedPath>::new();

            let auto_mounts = s.mounts.as_ref().map(|mounts| {
                mounts
                    .iter()
                    .map(|mount| match mount {
                        SidecarMount::Empty(mount) => {
                            VolumeBackedPath::config_data(workspace_key, mount, None, None, None)
                        }
                        SidecarMount::Files { mount, files } => VolumeBackedPath::config_data(
                            workspace_key,
                            mount,
                            Some(files.clone()),
                            None,
                            None,
                        ),
                    })
                    .collect::<Vec<_>>()
            });

            if let Some(v) = auto_mounts {
                mounts.extend_from_slice(&v.as_slice());
            }

            let work_mount = if let Some(true) = s.mount_work {
                Some(vec![VolumeBackedPath::work(workspace_key, work_dir)])
            } else {
                None
            };

            if let Some(v) = work_mount {
                mounts.extend_from_slice(&v.as_slice());
            }

            let uid = s.user.as_deref().unwrap_or(&constants::ROOT_UID);
            self.api
                .container
                .create(RunSpec {
                    reason: &container_name,
                    container_name: &container_name,
                    uid: &uid,
                    image: &s.image,
                    force_recreate: force,
                    workspace_key: &workspace_key,
                    labels,
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
                    args: s
                        .args
                        .as_ref()
                        .map(|x| x.iter().map(|z| z.as_ref()).collect()),
                    mounts: Some(
                        self.api
                            .volume
                            .ensure_mounts(&mounts, None, Some(uid))
                            .await?,
                    ),
                    ports: Some(ports),
                    work_dir: Some(s.work_dir.as_deref().unwrap_or(work_dir)),
                    run_mode: RunMode::Sidecar,
                    privileged: s.privileged.unwrap_or(false),
                    init: s.init.unwrap_or(true),
                    force_pull: pull_image,
                    ..Default::default()
                })
                .await?;
        }

        Ok(network.map(|n| n.to_string()))
    }
}
