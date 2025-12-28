use std::path::Path;

use crate::{
    api::WorkspaceApi,
    constants,
    model::{
        types::{AnyError, ContainerResult, RunMode, RunSpec, WorkSpec, WorkspaceResult},
        volume::VolumeBackedPath,
    },
    util::ssh,
};

impl<'a> WorkspaceApi<'a> {
    pub async fn create(&self, spec: &WorkSpec<'a>) -> Result<WorkspaceResult, AnyError> {
        let mut volumes = vec![VolumeBackedPath::work(spec.container_name, constants::WORK_DIR)];

        let home_dir = format!("/home/{}", &spec.user);
        if let Some(home_from_image) = spec.home_from_image {
            let home_vol = VolumeBackedPath::home(spec.container_name.into(), &home_dir);
            volumes.push(home_vol.clone());
            self.api
                .container
                .one_shot(
                    "populate-home",
                    "exit 0".into(),
                    Some(vec![home_vol.to_mount(None)]),
                    Some(spec.uid),
                    Some(home_from_image),
                )
                .await?;
        }

        if let Some(caches) = &spec.caches {
            log::debug!("Processing caches");
            let cache_vols = caches
                .iter()
                .map(|p| VolumeBackedPath::cache(p))
                .collect::<Vec<_>>();

            for c in caches {
                log::debug!("Cache: {}", c);
            }

            volumes.extend_from_slice(cache_vols.clone().as_slice());
        } else {
            log::debug!("No caches configured. Skipping");
        }

        let mut mounts = self
            .api
            .volume
            .ensure_mounts(&volumes, Some(&home_dir), Some(&spec.uid))
            .await?;

        mounts.push(ssh::mount(
            Path::new(&home_dir).join(".ssh").to_string_lossy().as_ref(),
        ));

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
            network: spec.network,
            env: spec.env_vars.clone(),
            ports: spec.ports.clone(),
            ..Default::default()
        };

        match self.api.container.create(run_spec).await? {
        ContainerResult::Created { .. } =>

            Ok(
                WorkspaceResult {
                    workspace_key: (&spec).workspace_key.to_string(),
                    working_dir: (&spec).container_working_dir.to_string(),
                    orig_uid: spec.uid.to_string(),
                    volumes: volumes.iter().map(|v|v.clone()).collect::<Vec<_>>() }),

        ContainerResult::AlreadyExists { .. } => {
            Err(format!("Container already exists. Did you mean: rooz enter {}? Otherwise, use --apply to reconfigure containers or --replace to recreate the whole workspace.", spec.workspace_key).into())
        }
    }
    }
}
