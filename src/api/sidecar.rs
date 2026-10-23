use crate::api::VolumeApi;
use crate::api::checkpoint::Checkpoints;
use crate::api::exec::InstallCheckpoints;
use crate::config::config::{MountSource, ResolvedStep};
use crate::config::runtime::RuntimeConfig;
use crate::model::types::{ContainerResult, VolumeFilesSpec};
use crate::{
    api::WorkspaceApi,
    config::config::RoozCfg,
    constants,
    model::types::{AnyError, RunMode, RunSpec},
    util::{
        labels::{self, Labels},
        live,
    },
};

use bollard_stubs::models::ContainerConfig;
use bollard_stubs::query_parameters::CommitContainerOptions;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

// A volume's files to write, owned by the sidecar's `uid` when it sets one
struct Population {
    files: VolumeFilesSpec,
    uid: Option<i32>,
}

impl<'a> WorkspaceApi<'a> {
    pub async fn ensure_sidecars(
        &self,
        config: &RuntimeConfig,
        workspace_key: &str,
        force: bool,
        pull_image: bool,
        rewrite_files: bool,
        recorded_images: Option<&HashMap<String, String>>,
    ) -> Result<RuntimeConfig, AnyError> {
        let mut cfg = config.clone();
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);
        let (_, peer_relations) = RuntimeConfig::workspace_networks(&config.sidecars);

        if cfg.sidecars.is_empty() {
            return Ok(cfg);
        }

        let volume_specs = VolumeApi::create_volume_specs(
            workspace_key,
            &config.data,
            &config.all_mounts(),
            false,
        )?;
        self.api.volume.ensure_volumes(&volume_specs).await?;
        let egress_network = &constants::egress_network(workspace_key);

        // populations targeting the same volume (e.g. the shared inline volume) must stay
        // sequential, as they chown/rewrite the same tree; distinct volumes are independent
        let mut by_volume: HashMap<String, Vec<Population>> = HashMap::new();

        for (name, s) in &mut cfg.sidecars {
            log::debug!("Process sidecar: {}", name);

            //TODO: read the uid from the sidecar image if not overridden by the user
            let uid = s.uid;

            let mounts: HashMap<String, MountSource> = s.mounts.clone();

            let mounts_all = mounts
                .iter()
                .map(|(target, source)| (target.to_string(), source.resolve_key(name, target)))
                .collect::<HashMap<String, String>>();

            let mounts_config =
                self.api
                    .volume
                    .mounts_with_sources(&volume_specs, &mounts_all, false)?;

            //TODO: not setting home dir as it depends on the user. When using uid the user might not
            // exist so it hard to make it work predictably. Consider marking as not supported by design
            let real_mounts = VolumeApi::real_mounts(mounts_config.clone(), None);

            for (t, m) in real_mounts.clone() {
                s.real_mounts.insert(t.clone(), m.clone());
                // The volume might already be created by the workspace-level volume creation
                // but still may need files in the paths not covered by that process
                by_volume
                    .entry(m.volume_name.as_str().to_string())
                    .or_default()
                    .push(Population { files: m, uid });
            }
        }

        // every population runs in a one-shot container of the default image: have it before they
        // run side by side, or each would pull it
        self.api
            .image
            .ensure(constants::DEFAULT_IMAGE, false)
            .await?;

        all_ok(
            futures::future::join_all(by_volume.into_values().map(|specs| async move {
                for p in specs {
                    self.api
                        .volume
                        .populate_volume(p.files, p.uid, true, rewrite_files)
                        .await?;
                }
                Ok(())
            }))
            .await,
        )?;

        // Volumes are populated above, one volume at a time per volume. The rest, building a sidecar's image above all, is independent of the other sidecars.
        let labels = &labels;
        let peer_relations = &peer_relations;
        let sidecars = cfg.sidecars.iter().map(|(name, s)| async move {
            let built: Result<(), AnyError> = async {
                let container_name = constants::sidecar_container(workspace_key, name);
                let mut labels = labels.clone();
                labels.extend(&[Labels::container(name), Labels::role(labels::SIDECAR_ROLE)]);
                let mut ports = HashMap::<String, Option<String>>::new();
                RoozCfg::parse_ports(&mut ports, s.ports.clone())?;
                let uid = s.uid;
                let container_mounts = self.api.volume.mounts(&s.real_mounts).await?;

                let cmd = &s.command.iter().map(|x| x.as_str()).collect::<Vec<_>>();
                let args = &s.args.iter().map(|k| k.as_str()).collect::<Vec<_>>();

                let uid_string = uid
                    .map(|x| x.to_string())
                    .unwrap_or(constants::ROOT_UID.to_string());

                let pair_network = &constants::pair_network(workspace_key, name);
                let mut extra_networks = peer_relations
                    .iter()
                    .filter(|(a, b)| a == name || b == name)
                    .map(|(a, b)| constants::peer_network(workspace_key, a, b))
                    .collect::<Vec<_>>();
                if s.egress {
                    extra_networks.push(egress_network.to_string());
                }
                let run_spec = RunSpec {
                    reason: &container_name,
                    container_name: &container_name,
                    uid: &uid_string,
                    image: &s.image,
                    force_recreate: force,
                    workspace_key,
                    labels: labels.clone(),
                    env: Some(s.env.clone()),
                    default_network: Some(pair_network),
                    additional_networks: if extra_networks.is_empty() {
                        None
                    } else {
                        Some(extra_networks.iter().map(|n| n.as_str()).collect())
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
                    mounts: Some(container_mounts),
                    ports: Some(ports),
                    work_dir: Some(s.work_dir.as_str()),
                    run_mode: RunMode::Sidecar,
                    privileged: s.privileged,
                    vpn: s.vpn,
                    init: s.init,
                    force_pull: pull_image,
                    ..Default::default()
                };

                let install_steps = s.install.as_ref().map(|i| i.resolved()).unwrap_or_default();
                if !install_steps.is_empty() {
                    let runtime_image = format!("localhost/rooz/{}/{}", workspace_key, name);
                    let base_image = self.api.image.ensure(&s.image, pull_image).await?;
                    let key = build_key(
                        &base_image.id,
                        &uid_string,
                        &install_steps,
                        &s.command,
                        &s.args,
                    );
                    let runtime_image_labels = Labels::from(&[
                        Labels::workspace(workspace_key),
                        Labels::role(labels::SIDECAR_RUNTIME_ROLE),
                        Labels::container(name),
                        Labels::build_key(&key),
                    ]);

                    // The tag is predictable and the image namespace is shared with every user of the
                    // engine, so the name is no evidence of what carries it - and neither are labels,
                    // which anyone tagging an image can write too. What rooz can rely on is the image
                    // id it recorded on this sidecar's container when it created it: an id is the
                    // image's content and cannot be pointed elsewhere. No recorded id (a new
                    // workspace, a removed container, a workspace from an older rooz) means the
                    // install stage runs again and the tag is rebuilt over. 'update' removes the
                    // containers first, so it passes on the ids it read from them. The build key
                    // label then decides whether that image still matches the config.
                    let mut image_id = match self.api.container.labels_of(&container_name).await? {
                        Some(found) if labels.is_subset_of(&found) => {
                            found.get(labels::RUNTIME_IMAGE).cloned()
                        }
                        _ => None,
                    }
                    .or_else(|| recorded_images.and_then(|r| r.get(name.as_str()).cloned()));

                    if let Some(id) = &image_id
                        && !self.api.image.is_pinned(id, &runtime_image_labels).await?
                    {
                        image_id = None;
                    }
                    if image_id.is_some() {
                        live::install_reused(name);
                    }

                    let checkpoints = Checkpoints::new(
                        workspace_key,
                        name,
                        &(&base_image.id, &uid_string, &s.command, &s.args),
                    );
                    let resume = if image_id.is_none() {
                        checkpoints.find(self.api.client, &install_steps).await
                    } else {
                        None
                    };

                    if image_id.is_none()
                        && let ContainerResult::Created { id: container_id } = self
                            .api
                            .container
                            .create(RunSpec {
                                run_mode: RunMode::SidecarInstall,
                                vpn: false,
                                default_network: Some(egress_network),
                                additional_networks: None,
                                // IMPORTANT: do not inject the sidecar env so it doesn't get baked into
                                // the runtime image. It also ensures unaltered behavior of the base image
                                // during the installation stage
                                env: None,
                                command: Some(vec!["sleep"]),
                                args: Some(vec!["infinity"]),
                                // pulled above already, and the build key holds that image's id
                                force_pull: false,
                                // the steps an earlier attempt finished are in this image
                                image: resume.as_ref().map_or(run_spec.image, |(_, id)| id),
                                ..run_spec.clone()
                            })
                            .await?
                    {
                        let built: Result<_, AnyError> = async {
                            self.api.container.start(&container_id).await?;
                            self.api
                                .exec
                                .install(
                                    name,
                                    &container_id,
                                    install_steps,
                                    Some(InstallCheckpoints {
                                        checkpoints: &checkpoints,
                                        done: resume.as_ref().map_or(0, |(done, _)| *done),
                                    }),
                                )
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
                                        labels: Some(runtime_image_labels.clone().into()),
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
                                .await
                                .map_err(AnyError::from)
                        }
                        .await;
                        // never left behind, built or not: a half-built install container carries
                        // the sidecar's name and labels and would pass for the sidecar itself
                        self.api.container.remove(&container_id, true).await?;
                        let built = built?;
                        checkpoints.clear(self.api.client).await;
                        image_id = Some(built.id);
                    }

                    // nothing to point at means the sidecar container is already there and was not
                    // recreated - the install container returned it as-is
                    if let Some(image_id) = image_id {
                        let mut labels = labels.clone();
                        labels.append(Labels::runtime_image(&image_id));
                        self.api
                            .container
                            .create(RunSpec {
                                image: &image_id,
                                labels,
                                // the image is local and identified by its content - there is
                                // nothing to pull and no tag a pull could move
                                force_pull: false,
                                ..run_spec.clone()
                            })
                            .await?;
                    }
                } else {
                    self.api.container.create(run_spec).await?;
                }
                Ok::<(), AnyError>(())
            }
            .await;
            built.map_err(|e| -> AnyError { format!("sidecar '{}': {}", name, e).into() })
        });
        // each sidecar runs to the end even when another fails: cut short, it would leave
        // half-done work behind
        all_ok(futures::future::join_all(sidecars).await)?;

        Ok(RuntimeConfig { ..cfg.clone() })
    }
}

// What a built sidecar image depends on: a change to any of these builds it again.
fn build_key(
    base_image_id: &str,
    uid: &str,
    steps: &[ResolvedStep],
    command: &[String],
    args: &[String],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{:?}", (base_image_id, uid, steps, command, args)));
    format!("{:x}", hasher.finalize())
}

// every failure, not just the first: all of them ran to the end
pub fn all_ok(results: impl IntoIterator<Item = Result<(), AnyError>>) -> Result<(), AnyError> {
    let errors = results
        .into_iter()
        .filter_map(Result::err)
        .map(|e| e.to_string())
        .collect::<Vec<_>>();
    match errors.len() {
        0 => Ok(()),
        _ => Err(errors.join("\n").into()),
    }
}
