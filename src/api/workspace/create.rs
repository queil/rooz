use std::path::Path;

use crate::{
    api::{container, WorkspaceApi},
    constants,
    model::{
        types::{AnyError, ContainerResult, RunMode, RunSpec, WorkSpec, WorkspaceResult},
        volume::RoozVolume,
    },
    util::ssh,
};

impl<'a> WorkspaceApi<'a> {
    pub async fn create(&self, spec: &WorkSpec<'a>) -> Result<WorkspaceResult, AnyError> {
        let mut volumes = vec![RoozVolume::work(spec.container_name, constants::WORK_DIR)];

        let auto_mounts = spec.mounts.as_ref().map(|mounts| {
            mounts
                .iter()
                .map(|mount| mount.to_volume(spec.workspace_key))
                .collect::<Vec<_>>()
        });

        if let Some(v) = auto_mounts {
            volumes.extend_from_slice(&v.as_slice());
        }

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

        let symlink_paths = volumes
            .iter()
            .filter(|v| v.file.is_some())
            .map(|v| v.to_mount(Some(&home_dir)).target.unwrap())
            .collect::<Vec<_>>();

        let make_dirs = format!(
            "for f in {}; do [ ! -e $f ] && mkdir -p $(dirname $f) && ln -s {}$f $f; done",
            &symlink_paths.join(" "),
            constants::ROOZ_DATA_DIR,
        );

        let entrypoint = container::inject(
            &vec![make_dirs, "cat".to_string()].join(" && "),
            "entrypoint.sh",
        );

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
            entrypoint: Some((&entrypoint).iter().map(|f| f.as_str()).collect::<_>()),
            privileged: spec.privileged,
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
