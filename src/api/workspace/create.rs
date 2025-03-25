use std::path::Path;

use crate::{
    api::WorkspaceApi,
    constants,
    model::{
        types::{AnyError, ContainerResult, RunSpec, WorkSpec, WorkspaceResult},
        volume::RoozVolume,
    },
    util::ssh,
};

impl<'a> WorkspaceApi<'a> {
    pub async fn create(&self, spec: &WorkSpec<'a>) -> Result<WorkspaceResult, AnyError> {
        let home_dir = format!("/home/{}", &spec.user);

        let mut volumes = vec![
            RoozVolume::home(spec.container_name.into(), &home_dir),
            RoozVolume::work(spec.container_name, constants::WORK_DIR),
        ];

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

        let mut mounts = self
            .api
            .volume
            .ensure_mounts(&volumes, Some(&home_dir))
            .await?;

        mounts.push(ssh::mount(
            Path::new(&home_dir).join(".ssh").to_string_lossy().as_ref(),
        ));

        mounts.push(
            self.crypt
                .mount(Path::new(&home_dir).join(".age").to_string_lossy().as_ref()),
        );

        let run_spec = RunSpec {
            reason: "work",
            image: &spec.image,
            uid: spec.uid,
            user: &spec.user,
            work_dir: Some(&spec.container_working_dir),
            home_dir: &home_dir,
            container_name: &spec.container_name,
            workspace_key: &spec.workspace_key,
            mounts: Some(mounts),
            entrypoint: spec.entrypoint.clone(),
            privileged: spec.privileged,
            force_recreate: spec.force_recreate,
            auto_remove: spec.ephemeral,
            labels: spec.labels.clone(),
            network: spec.network,
            env: spec.env_vars.clone(),
            ports: spec.ports.clone(),
            interactive: true,
            ..Default::default()
        };

        match self.api.container.create(run_spec).await? {
        ContainerResult::Created { .. } =>

            Ok(
                WorkspaceResult {
                    workspace_key: (&spec).workspace_key.to_string(),
                    working_dir: (&spec).container_working_dir.to_string(),
                    orig_uid: spec.uid,
                    volumes: volumes.iter().map(|v|v.clone()).collect::<Vec<_>>() }),

        ContainerResult::AlreadyExists { .. } => {
            Err(format!("Container already exists. Did you mean: rooz enter {}? Otherwise, use --apply to reconfigure containers or --replace to recreate the whole workspace.", spec.workspace_key).into())
        }
    }
    }
}
