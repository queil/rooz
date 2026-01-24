use crate::config::config::{DataEntry};
use crate::model::types::VolumeSpec;
use crate::util::id;
use crate::util::labels::{DATA_ROLE, Labels};
use crate::{
    api::WorkspaceApi,
    constants,
    model::{
        types::{AnyError, ContainerResult, RunMode, RunSpec, WorkSpec, WorkspaceResult},
        volume::RoozVolume,
    },
    util::ssh,
};
use bollard_stubs::models::Mount;
use bollard_stubs::models::MountTypeEnum::VOLUME;
use std::path::Path;

impl<'a> WorkspaceApi<'a> {

    fn expand_home(path: String, home: Option<&str>) -> String {
        match (home, path.strip_prefix("~/")) {
            (Some(h), Some(rest)) => format!("{}/{}", h, rest),
            (Some(h), None) if path == "~" => h.to_string(),
            _ => path.clone(),
        }
    }
    pub async fn create(&self, spec: &WorkSpec<'a>) -> Result<WorkspaceResult, AnyError> {
        // ---- VOLUMES v2 impl ----
        // here each DataEntry case needs different handling
        // leaving these excessive comments here so I can resume the work after a break

        // make volume specs
        let volumes_v2 = if let Some(data) = &spec.data {
            data.iter()
                .map(|d| match d {
                    DataEntry::Dir { name } => VolumeSpec {
                        name: format!(
                            "rooz-{}-{}",
                            id::sanitize(spec.workspace_key),
                            id::sanitize(name)
                        ),
                        labels: Some(Labels::from(&[
                            Labels::workspace(spec.workspace_key),
                            Labels::role(DATA_ROLE),
                        ])),
                    },
                    _ => panic!("Not implemented yet"),
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        // make volumes

        for v in volumes_v2 {
            self.api.volume.ensure_volume_v2(&v).await?;
        }

        let home_dir = format!("/home/{}", &spec.user);

        //TODO: verify if source is actually declared in `data`
        let mounts_v2 = if let Some(mounts) = spec.mounts.clone() {
            mounts
                .into_iter()
                .map(|(target, source)| Mount {
                    target: Some(Self::expand_home(target, Some(&home_dir))),
                    source: Some(source),
                    typ: Some(VOLUME),
                    read_only: Some(false),
                    ..Mount::default()
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };



        //TODO: initialize volumes according to the DataEntry type

        //TODO: NEXT STEPS
        //TODO: 1. all great but we need to make sure the volumes v2 are chown'd, that happens on enter
        // so need to refactor there.

        //TODO: 2. in tmp mode we delete volumes exclusive to the workspace so that must be retained

        //TODO: 3. all built-in stuff must be included in v2 - caches, ssh, system-config, work, etc.

        // ---- END VOLUMES v2 impl ----

        let mut volumes = vec![RoozVolume::work(spec.container_name, constants::WORK_DIR)];

        // TODO: in v2 is just a special case of image
        // if let Some(home_from_image) = spec.home_from_image {
        //     let home_vol = RoozVolume::home(spec.container_name.into(), &home_dir);
        //     volumes.push(home_vol.clone());
        //     self.api
        //         .container
        //         .one_shot(
        //             "populate-home",
        //             "exit 0".into(),
        //             Some(vec![home_vol.to_mount(None)]),
        //             Some(spec.uid),
        //             Some(home_from_image),
        //         )
        //         .await?;
        // }

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
            .ensure_mounts(&volumes, Some(&home_dir), Some(&spec.uid))
            .await?;

        mounts.push(ssh::mount(
            Path::new(&home_dir).join(".ssh").to_string_lossy().as_ref(),
        ));

        mounts.extend(mounts_v2);

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
            ContainerResult::Created { .. } => Ok(WorkspaceResult {
                workspace_key: (&spec).workspace_key.to_string(),
                working_dir: (&spec).container_working_dir.to_string(),
                orig_uid: spec.uid.to_string(),
                volumes: volumes.iter().map(|v| v.clone()).collect::<Vec<_>>(),
            }),

            ContainerResult::AlreadyExists { .. } => {
                Err(format!("Workspace {} already exists.", spec.workspace_key).into())
            }
        }
    }
}
