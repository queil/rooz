use std::collections::HashMap;

use crate::api::VolumeApi;
use crate::config::config::MountSource;
use crate::config::runtime::RuntimeConfig;
use crate::{
    api::WorkspaceApi,
    config::config::RoozCfg,
    model::types::{AnyError, RunMode, RunSpec},
    util::labels::{self, Labels},
};
use bollard::models::NetworkCreateRequest;

impl<'a> WorkspaceApi<'a> {
    pub async fn ensure_sidecars(
        &self,
        config: &RuntimeConfig,
        workspace_key: &str,
        force: bool,
        pull_image: bool,
    ) -> Result<(RuntimeConfig, Option<String>), AnyError> {
        let mut cfg = config.clone();
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);

        let network = if !cfg.sidecars.is_empty() {
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

        for (name, s) in &mut cfg.sidecars {
            log::debug!("Process sidecar: {}", name);
            let container_name = format!("{}-{}", workspace_key, name);
            let mut labels = labels.clone();
            labels.extend(&[Labels::container(&name), Labels::role(labels::SIDECAR_ROLE)]);
            let mut ports = HashMap::<String, Option<String>>::new();
            RoozCfg::parse_ports(&mut ports, s.ports.clone());

            let mounts: HashMap<String, MountSource> = s.mounts.clone();

            let volumes_v2 = VolumeApi::create_volume_specs(workspace_key, &config.data, &mounts);

            self.api.volume.ensure_volumes_v2(&volumes_v2).await?;

            let mut mounts_v2 = Vec::new();

            let mounts_all = mounts
                .iter()
                .map(|(target, source)| (target.to_string(), source.resolve_key(target)))
                .collect::<HashMap<String, String>>();

            let mounts_config = self
                .api
                .volume
                .mounts_with_sources(&volumes_v2, &mounts_all);

            let real_mounts = VolumeApi::real_mounts_v2(mounts_config.clone(), None);

            mounts_v2.extend_from_slice(self.api.volume.mounts_v2(&real_mounts).await?.as_slice());

            for (t, m) in real_mounts {
                s.real_mounts.insert(t.clone(), m.clone());
                // The volume might already be created by the workspace-level volume creation
                // but still may need files in the paths not covered by that process
                self.api.volume.populate_volume(t, m, None).await?;
            }

            let uid = s.user.clone();
            let cmd = &s.command.iter().map(|x| x.as_str()).collect::<Vec<_>>();
            let args = &s.args.iter().map(|k| k.as_str()).collect::<Vec<_>>();
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
                    env: Some(s.env.clone()),
                    network,
                    network_aliases: Some(vec![name.into()]),
                    command: Some(cmd.clone()),
                    args: Some(args.clone()),
                    mounts: Some(mounts_v2),
                    ports: Some(ports),
                    work_dir: Some(s.work_dir.as_str()),
                    run_mode: RunMode::Sidecar,
                    privileged: s.privileged,
                    init: s.init,
                    force_pull: pull_image,
                    ..Default::default()
                })
                .await?;
        }

        Ok((
            RuntimeConfig { ..cfg.clone() },
            network.map(|n| n.to_string()),
        ))
    }
}
