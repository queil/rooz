use crate::api::VolumeApi;
use crate::config::config::MountSource;
use crate::config::runtime::RuntimeConfig;
use crate::model::types::ContainerResult;
use crate::{
    api::WorkspaceApi,
    config::config::RoozCfg,
    constants,
    model::types::{AnyError, RunMode, RunSpec},
    util::labels::{self, Labels},
};

use bollard_stubs::models::ContainerConfig;
use bollard_stubs::query_parameters::CommitContainerOptions;
use std::collections::HashMap;

impl<'a> WorkspaceApi<'a> {
    pub async fn ensure_sidecars(
        &self,
        config: &RuntimeConfig,
        workspace_key: &str,
        force: bool,
        pull_image: bool,
    ) -> Result<RuntimeConfig, AnyError> {
        let mut cfg = config.clone();
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);

        for (name, s) in &mut cfg.sidecars {
            log::debug!("Process sidecar: {}", name);
            let container_name = format!("{}-{}", workspace_key, name);
            let mut labels = labels.clone();
            labels.extend(&[Labels::container(&name), Labels::role(labels::SIDECAR_ROLE)]);
            let mut ports = HashMap::<String, Option<String>>::new();
            RoozCfg::parse_ports(&mut ports, s.ports.clone());

            //TODO: read the uid from the sidecar image if not overridden by the user
            let uid = s.uid.clone();

            let mounts: HashMap<String, MountSource> = s.mounts.clone();

            let volumes_v2 = VolumeApi::create_volume_specs(
                workspace_key,
                &config.data,
                &config.all_mounts(),
                false,
            );

            self.api.volume.ensure_volumes_v2(&volumes_v2).await?;

            let mut mounts_v2 = Vec::new();

            let mounts_all = mounts
                .iter()
                .map(|(target, source)| (target.to_string(), source.resolve_key(target)))
                .collect::<HashMap<String, String>>();

            let mounts_config =
                self.api
                    .volume
                    .mounts_with_sources(&volumes_v2, &mounts_all, false);

            //TODO: not setting home dir as it depends on the user. When using uid the user might not
            // exist so it hard to make it work predictably. Consider marking as not supported by design
            let real_mounts = VolumeApi::real_mounts_v2(mounts_config.clone(), None);

            mounts_v2.extend_from_slice(self.api.volume.mounts_v2(&real_mounts).await?.as_slice());

            for (t, m) in real_mounts.clone() {
                s.real_mounts.insert(t.clone(), m.clone());
                // The volume might already be created by the workspace-level volume creation
                // but still may need files in the paths not covered by that process
                self.api.volume.populate_volume(t, m, uid).await?;
            }

            let cmd = &s.command.iter().map(|x| x.as_str()).collect::<Vec<_>>();
            let args = &s.args.iter().map(|k| k.as_str()).collect::<Vec<_>>();

            let uid_string = uid
                .map(|x| x.to_string())
                .unwrap_or(constants::ROOT_UID.to_string());

            let internal_network = &constants::internal_network(workspace_key);
            let egress_network = &constants::egress_network(workspace_key);
            let run_spec = RunSpec {
                reason: &container_name,
                container_name: &container_name,
                uid: &uid_string,
                image: &s.image,
                force_recreate: force,
                workspace_key: &workspace_key,
                labels: labels.clone(),
                env: Some(s.env.clone()),
                default_network: Some(internal_network),
                additional_networks: if s.egress {
                    Some(vec![egress_network])
                } else {
                    None
                },
                network_aliases: Some(vec![name.into()]),
                command: if cmd.is_empty() {
                    None
                } else {
                    Some(cmd.clone())
                },
                args: if args.is_empty() {
                    None
                } else {
                    Some(args.clone())
                },
                mounts: Some(mounts_v2),
                ports: Some(ports),
                work_dir: Some(s.work_dir.as_str()),
                run_mode: RunMode::Sidecar,
                privileged: s.privileged,
                init: s.init,
                force_pull: pull_image,
                ..Default::default()
            };

            if let Some(install) = s.install.clone() {
                let runtime_image = format!("localhost/rooz/{}/{}", &workspace_key, &name);

                if !self.api.image.exists(&runtime_image).await? {
                    if let ContainerResult::Created { id: container_id } = self
                        .api
                        .container
                        .create(RunSpec {
                            run_mode: RunMode::SidecarInstall,
                            default_network: Some(egress_network),
                            // IMPORTANT: do not inject the sidecar env so it doesn't get baked into
                            // the runtime image. It also ensures unaltered behavior of the base image
                            // during the installation stage
                            env: None,
                            command: Some(vec!["sleep"]),
                            args: Some(vec!["infinity"]),
                            ..run_spec.clone()
                        })
                        .await?
                    {
                        // IMPORTANT: make symlinks to the mounted volumes before install so the
                        // mounted files/dirs are available
                        self.api
                            .container
                            .symlink_files(&container_id, &real_mounts, uid)
                            .await?;
                        self.api.container.start(&container_id).await?;
                        self.api
                            .exec
                            .install(&container_name, &container_id, install)
                            .await?;
                        self.api
                            .container
                            .client
                            .commit_container(
                                CommitContainerOptions {
                                    container: Some(container_id.clone()),
                                    repo: Some(runtime_image.to_string()),
                                    tag: Some("latest".to_string()),
                                    pause: false,
                                    ..Default::default()
                                },
                                ContainerConfig {
                                    cmd: if s.args.is_empty() {
                                        None
                                    } else {
                                        Some(s.args.clone())
                                    },
                                    entrypoint: if s.command.is_empty() {
                                        None
                                    } else {
                                        Some(s.command.clone())
                                    },
                                    ..Default::default()
                                },
                            )
                            .await?;
                        self.api.container.stop(&container_id).await?;
                        self.api.container.remove(&container_id, true).await?;
                    }
                }

                let latest_runtime_image = format!("{}:latest", runtime_image);
                if let ContainerResult::Created { id: container_id } = self
                    .api
                    .container
                    .create(RunSpec {
                        image: &latest_runtime_image,
                        ..run_spec.clone()
                    })
                    .await?
                {
                    self.api
                        .container
                        .symlink_files(&container_id, &real_mounts, uid)
                        .await?;
                }
            } else {
                if let ContainerResult::Created { id: container_id } =
                    self.api.container.create(run_spec).await?
                {
                    self.api
                        .container
                        .symlink_files(&container_id, &real_mounts, uid)
                        .await?;
                }
            }
        }

        Ok(RuntimeConfig { ..cfg.clone() })
    }
}
