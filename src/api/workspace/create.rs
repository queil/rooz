use crate::{
    api::{WorkspaceApi, checkpoint::Checkpoints, exec::InstallCheckpoints, sidecar::run_all},
    model::{
        types::{AnyError, ContainerResult, RunMode, RunSpec, WorkSpec, WorkspaceResult},
        volume::RoozVolume,
    },
    util::ssh,
};
use std::path::Path;

impl<'a> WorkspaceApi<'a> {
    pub async fn create(&self, spec: &WorkSpec<'a>) -> Result<WorkspaceResult, AnyError> {
        let mut volumes = vec![];

        if let Some(caches) = &spec.caches {
            log::debug!("Processing caches");
            let cache_vols = caches
                .iter()
                .map(|p| RoozVolume::cache(p))
                .collect::<Vec<_>>();

            for c in caches {
                log::debug!("Cache: {}", c);
            }

            volumes.extend_from_slice(cache_vols.clone().as_slice());
        } else {
            log::debug!("No caches configured. Skipping");
        }
        let home_dir = format!("/home/{}", spec.user);
        let mut mounts = self
            .api
            .volume
            .ensure_mounts(&volumes, Some(&home_dir))
            .await?;

        self.api.volume.require_ssh_key().await?;
        mounts.push(ssh::mount(
            Path::new(&home_dir).join(".ssh").to_string_lossy().as_ref(),
            true,
        ));

        mounts.extend(spec.mounts.clone());

        let writable_targets = mounts
            .iter()
            .filter(|m| m.read_only != Some(true))
            .filter_map(|m| m.target.clone())
            .collect::<Vec<_>>();

        let install_steps = run_all(
            spec.install
                .as_ref()
                .map(|i| i.resolved())
                .unwrap_or_default(),
            spec.run_all_steps,
        );
        // An ephemeral workspace is not rebuilt, so there is nothing to resume it from.
        // The environment is hashed with the rest: the steps run with it.
        let checkpoints = if install_steps.is_empty() || spec.ephemeral {
            None
        } else {
            let base = self.api.image.ensure(spec.image, false).await?;
            let env = spec
                .env_vars
                .iter()
                .flatten()
                .collect::<std::collections::BTreeMap<_, _>>();
            Some(Checkpoints::new(
                spec.workspace_key,
                spec.container_name,
                &(
                    &base.id,
                    spec.uid,
                    spec.user,
                    &spec.command,
                    &spec.args,
                    env,
                ),
            ))
        };
        let resume = match &checkpoints {
            Some(c) => c.find(self.api.client, &install_steps).await,
            None => None,
        };

        let run_spec = RunSpec {
            reason: "work",
            // the steps an earlier attempt finished are in this image
            image: resume.as_ref().map_or(spec.image, |(_, id)| id),
            uid: spec.uid,
            user: spec.user,
            work_dir: Some(spec.container_working_dir),
            home_dir: &home_dir,
            container_name: spec.container_name,
            workspace_key: spec.workspace_key,
            mounts: Some(mounts),
            command: spec.command.clone(),
            args: spec.args.clone(),
            privileged: spec.privileged,
            init: spec.init,
            force_recreate: spec.force_recreate,
            run_mode: if spec.ephemeral {
                RunMode::Tmp
            } else {
                RunMode::Workspace
            },
            labels: spec.labels.clone(),
            default_network: spec.default_network,
            additional_networks: spec.additional_networks.clone(),
            env: spec.env_vars.clone(),
            ports: spec.ports.clone(),
            ..Default::default()
        };

        match self.api.container.create(run_spec).await? {
            ContainerResult::Created { id: container_id } => {
                if !install_steps.is_empty() {
                    self.api.container.start(&container_id).await?;
                    if install_steps.iter().any(|step| !step.root) {
                        // steps run as the workspace user, which has to exist and own the
                        // volumes it writes to - enter does the same before the first shell
                        self.api.exec.ensure_user(&container_id).await?;
                        let uid = spec.uid.parse::<i32>()?;
                        for target in &writable_targets {
                            self.api.exec.chown(&container_id, &uid, target).await?;
                        }
                    }
                    self.api
                        .exec
                        .install(
                            spec.container_name,
                            &container_id,
                            install_steps,
                            checkpoints.as_ref().map(|c| InstallCheckpoints {
                                checkpoints: c,
                                done: resume.as_ref().map_or(0, |(done, _)| *done),
                            }),
                        )
                        .await?;
                    if let Some(c) = &checkpoints {
                        c.clear(self.api.client).await;
                    }
                }

                Ok(WorkspaceResult {
                    workspace_key: spec.workspace_key.to_string(),
                    working_dir: spec.container_working_dir.to_string(),
                })
            }

            ContainerResult::AlreadyExists { .. } => {
                Err(format!("Workspace {} already exists.", spec.workspace_key).into())
            }
        }
    }
}
