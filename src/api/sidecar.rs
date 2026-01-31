use std::collections::HashMap;

use crate::api::VolumeApi;
use crate::config::config::{DataValue, MountSource, SidecarMount};
use crate::model::types::{DataEntryKey, DataEntryVolumeSpec};
use crate::model::volume::RoozVolume;
use crate::util::id;
use crate::{
    api::WorkspaceApi,
    config::config::{RoozCfg, RoozSidecar},
    constants,
    model::types::{AnyError, RunMode, RunSpec},
    util::labels::{self, Labels},
};
use bollard::models::NetworkCreateRequest;
use bollard_stubs::models::Mount;

impl<'a> WorkspaceApi<'a> {
    pub async fn ensure_sidecars(
        &self,
        sidecars: &HashMap<String, RoozSidecar>,
        data: &HashMap<String, DataValue>,
        workspace_key: &str,
        force: bool,
        pull_image: bool,
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

            let volumes_v2 = VolumeApi::create_volume_specs(
                workspace_key,
                &s.mounts
                    .clone()
                    .unwrap_or_default()
                    .iter()
                    .map(|(k, v)| match v {
                        MountSource::DataEntryReference(data_key) => (
                            data_key.as_str().to_string(),
                            {
                                let source_exists = data.contains_key(data_key.as_str());
                                if !source_exists {
                                    panic!(
                                        "Key '{}' not found under 'data:' in workspace config. Keys: {:?}",
                                        data_key.as_str(),
                                        &data.keys(),
                                    );
                                }

                                data[data_key.as_str()].clone()
                            },
                        ),
                        MountSource::InlineDataValue(data_value) => {
                            (id::sanitize(k), data_value.to_owned())
                        }
                    })
                    .collect::<HashMap<String, DataValue>>(),
            );

            let mut mounts_v2 = Vec::new();

            let mounts_all = &s
                .mounts
                .clone()
                .unwrap_or_default()
                .clone()
                .iter()
                .map(|(k, v)| match v {
                    MountSource::DataEntryReference(data_key) => {
                        (k.to_string(), data_key.as_str().to_string())
                    }
                    MountSource::InlineDataValue(_) => (k.to_string(), id::sanitize(k)),
                })
                .collect::<HashMap<String, String>>();

            let mounts_config = self
                .api
                .volume
                .mounts_with_sources(&volumes_v2, &mounts_all);

            let real_mounts = VolumeApi::real_mounts_v2(mounts_config.clone(), None);

            mounts_v2.extend_from_slice(self.api.volume.mounts_v2(&real_mounts).await?.as_slice());

            let uid = s.user.as_deref().unwrap_or(&constants::ROOT_UID);

            //TODO: remove LEGACY INLINE MOUNTS - v2 will handle that via implicit anonymous mounts

            let legacy_rooz_vols = s.legacy_mounts.as_ref().map(|mounts| {
                mounts
                    .iter()
                    .map(|mount| match mount {
                        SidecarMount::Empty(mount) => {
                            RoozVolume::config_data(workspace_key, mount, None, None, None)
                        }
                        SidecarMount::Files { mount, files } => RoozVolume::config_data(
                            workspace_key,
                            mount,
                            Some(files.clone()),
                            None,
                            None,
                        ),
                    })
                    .collect::<Vec<_>>()
            });

            if let Some(vols) = legacy_rooz_vols {
                let ms = self
                    .api
                    .volume
                    .ensure_mounts(&vols, None, Some(uid))
                    .await?;

                mounts_v2.extend_from_slice(ms.as_slice());
            }
            // END - LEGACY INLINE MOUNTS

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
                    mounts: Some(mounts_v2),
                    ports: Some(ports),
                    work_dir: s.work_dir.as_deref(),
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
