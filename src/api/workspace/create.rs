use crate::model::types::{TargetDir, VolumeFilesSpec};
use crate::{
    api::WorkspaceApi,
    model::{
        types::{AnyError, ContainerResult, RunMode, RunSpec, WorkSpec, WorkspaceResult},
        volume::RoozVolume,
    },
    util::ssh,
};
use std::collections::HashMap;
use std::path::Path;

impl<'a> WorkspaceApi<'a> {
    pub async fn create(
        &self,
        spec: &WorkSpec<'a>,
        real_mounts: &HashMap<TargetDir, VolumeFilesSpec>,
    ) -> Result<WorkspaceResult, AnyError> {
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
        let home_dir = format!("/home/{}", &spec.user);
        let mut mounts = self
            .api
            .volume
            .ensure_mounts(&volumes, Some(&home_dir), Some(&spec.uid))
            .await?;

        mounts.push(ssh::mount(
            Path::new(&home_dir).join(".ssh").to_string_lossy().as_ref(),
        ));

        mounts.extend(spec.mounts.clone());

        let run_spec = RunSpec {
            reason: "work",
            image: &spec.image,
            uid: &spec.uid,
            user: &spec.user,
            work_dir: Some(&spec.container_working_dir),
            home_dir: &home_dir,
            container_name: &spec.container_name,
            workspace_key: &spec.workspace_key,
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
            networks: spec.network.clone(),
            internet_access: true,
            env: spec.env_vars.clone(),
            ports: spec.ports.clone(),
            ..Default::default()
        };

        match self.api.container.create(run_spec).await? {
            ContainerResult::Created { id: container_id } => {
                self.api
                    .container
                    .symlink_files(&container_id, &real_mounts, Some(spec.uid.parse::<i32>()?))
                    .await?;
                if let Some(install) = spec.install.clone() {
                    self.api.container.start(&container_id).await?;
                    self.api
                        .exec
                        .install(spec.container_name, &container_id, install)
                        .await?;
                }

                Ok(WorkspaceResult {
                    workspace_key: (&spec).workspace_key.to_string(),
                    working_dir: (&spec).container_working_dir.to_string(),
                })
            }

            ContainerResult::AlreadyExists { .. } => {
                Err(format!("Workspace {} already exists.", spec.workspace_key).into())
            }
        }
    }
}
