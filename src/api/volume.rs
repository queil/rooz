use std::collections::HashMap;
use std::path::Path;

use crate::config::config::{DataEntry, DataExt, DataValue};
use crate::model::types::{
    DataEntryKey, DataEntryVolumeSpec, TargetDir, TargetPath, VolumeName, VolumeSpec,
};
use crate::util::id;
use crate::util::labels::DATA_ROLE;
use crate::{
    api::VolumeApi,
    model::{
        types::{AnyError, VolumeResult},
        volume::{RoozVolume, RoozVolumeFile},
    },
    util::labels::Labels,
};
use base64::{Engine as _, engine::general_purpose};
use bollard::{
    errors::Error::DockerResponseServerError,
    models::{Volume, VolumeCreateOptions},
    query_parameters::{ListVolumesOptions, RemoveVolumeOptions},
    service::Mount,
};
use bollard_stubs::models::MountTypeEnum::VOLUME;

impl<'a> VolumeApi<'a> {
    pub async fn get_all(&self, labels: &Labels) -> Result<Vec<Volume>, AnyError> {
        let list_options = ListVolumesOptions {
            filters: Some(labels.clone().into()),
            ..Default::default()
        };

        Ok(self
            .client
            .list_volumes(Some(list_options))
            .await?
            .volumes
            .unwrap_or_default())
    }

    pub async fn get_single(&self, labels: &Labels) -> Result<Option<Volume>, AnyError> {
        match self.get_all(&labels).await?.as_slice() {
            [] => Ok(None),
            [volume] => Ok(Some(volume.clone())),
            _ => panic!("Too many volumes found"),
        }
    }

    async fn create_volume(&self, options: VolumeCreateOptions) -> Result<VolumeResult, AnyError> {
        match &self.client.create_volume(options).await {
            Ok(v) => {
                log::debug!("Volume created: {:?}", v.name);
                Ok(VolumeResult::Created)
            }
            Err(e) => panic!("{}", e),
        }
    }

    pub async fn remove_volume(&self, name: &str, force: bool) -> Result<(), AnyError> {
        let options = RemoveVolumeOptions { force };
        match &self.client.remove_volume(name, Some(options)).await {
            Ok(_) => {
                let force_display = if force { " (force)" } else { "" };
                log::debug!("Volume removed: {} {}", &name, &force_display);
                Ok(())
            }
            Err(e) => panic!("{}", e),
        }
    }

    pub async fn ensure_volume_v2(&self, spec: &VolumeSpec) -> Result<VolumeResult, AnyError> {
        match self.client.inspect_volume(&spec.name).await {
            Ok(_) => {
                log::debug!("Reusing an existing {} volume", &spec.name);
                Ok(VolumeResult::AlreadyExists)
            }
            Err(DockerResponseServerError {
                status_code: 404,
                message: _,
            }) => {
                self.create_volume(VolumeCreateOptions {
                    name: Some(spec.name.to_string()),
                    labels: spec.labels.clone().map(|x| x.into()),
                    ..Default::default()
                })
                .await
            }
            Err(e) => panic!("{}", e),
        }
    }

    fn expand_home(path: String, home: Option<&str>) -> String {
        match (home, path.strip_prefix("~/")) {
            (Some(h), Some(rest)) => format!("{}/{}", h, rest),
            (Some(h), None) if path == "~" => h.to_string(),
            _ => path.clone(),
        }
    }

    pub fn mounts_with_sources(
        &self,
        volumes: &HashMap<DataEntryKey, DataEntryVolumeSpec>,
        mounts: &HashMap<String, String>,
    ) -> HashMap<TargetPath, DataEntryVolumeSpec> {
        let mut result = HashMap::new();
        for (target, source_key) in mounts {
            let source_key = DataEntryKey(source_key.to_string());
            let source_exists = volumes.contains_key(&source_key);
            if !source_exists {
                panic!(
                    "Key '{}' not found under 'data:' in workspace config. Keys: {:?}",
                    source_key.as_str(),
                    &volumes.keys(),
                );
            }
            result.insert(TargetPath(target.clone()), volumes[&source_key].clone());
        }
        result
    }
    fn volume_name(workspace_key: &str, data_entry_name: &str) -> String {
        format!(
            "rooz-{}-{}",
            id::sanitize(workspace_key),
            id::sanitize(data_entry_name)
        )
    }
    pub fn create_volume_specs(
        workspace_key: &str,
        data: &HashMap<String, DataValue>,
    ) -> HashMap<DataEntryKey, DataEntryVolumeSpec> {
        let mut data_entries = vec![];
        data_entries.extend_from_slice(data.clone().into_entries().as_slice());

        let files_key = "files";
        let files_volume_spec = VolumeSpec {
            name: Self::volume_name(workspace_key, files_key),
            labels: Some(Labels::from(&[
                Labels::workspace(workspace_key),
                Labels::role(DATA_ROLE),
            ])),
        };

        data_entries
            .iter()
            .filter_map(|d| match d {
                DataEntry::Dir { name } => Some((
                    DataEntryKey(name.to_string()),
                    DataEntryVolumeSpec {
                        data: d.clone(),
                        volume: VolumeSpec {
                            name: Self::volume_name(workspace_key, name),
                            labels: Some(Labels::from(&[
                                Labels::workspace(workspace_key),
                                Labels::role(DATA_ROLE),
                            ])),
                        },
                    },
                )),
                DataEntry::File { name, .. } => Some((
                    DataEntryKey(name.to_string()),
                    DataEntryVolumeSpec {
                        data: d.clone(),
                        volume: files_volume_spec.clone(),
                    },
                )),
                _ => panic!("Not implemented yet"),
            })
            .collect::<HashMap<_, _>>()
    }

    pub fn real_mounts_v2(
        mounts: HashMap<TargetPath, DataEntryVolumeSpec>,
        home_dir: Option<&str>,
    ) -> HashMap<TargetDir, VolumeName> {
        mounts
            .iter()
            .map(|(target, source_entry)| {
                let expanded_target = Self::expand_home(target.as_str().to_string(), home_dir);
                // remap '/' to '/var/lib/rooz' and drop the file name
                let real_target = match source_entry.data {
                    DataEntry::File { .. } => Path::new("/var/lib/rooz")
                        .join(
                            Path::new(&expanded_target)
                                .parent()
                                .unwrap()
                                .to_string_lossy()
                                .trim_start_matches('/'),
                        )
                        .to_string_lossy()
                        .into_owned(),
                    _ => expanded_target,
                };
                (
                    TargetDir(real_target.to_string()),
                    VolumeName(source_entry.volume.name.to_string()),
                )
            })
            .collect::<HashMap<_, _>>()
    }
    pub async fn ensure_volumes_v2(
        &self,
        data_entries: &HashMap<DataEntryKey, DataEntryVolumeSpec>,
    ) -> Result<(), AnyError> {
        for d in data_entries
            .iter()
            .map(|(_, v)| (v.volume.name.clone(), v.volume.clone()))
            .collect::<HashMap<_, _>>()
            .into_values()
        {
            self.ensure_volume_v2(&d).await?;
        }
        Ok(())
    }

    pub async fn populate_volumes_v2(&self, mount_specs: &HashMap<TargetDir, DataEntryVolumeSpec>) {
        for (target, v) in mount_specs {
            // self.ensure_file(&v.volume.name, target, vec![RoozVolumeFile{ file_path: "".to_string(), data: "".to_string() }] )
        }
    }

    pub async fn mounts_v2(
        &self,
        real_mounts: &HashMap<TargetDir, VolumeName>,
    ) -> Result<Vec<Mount>, AnyError> {
        let mut mount_entries = HashMap::new();
        mount_entries.extend(real_mounts.clone());

        let mounts_v2 = mount_entries
            .into_iter()
            .map(|(target, source)| Mount {
                target: Some(target.as_str().to_string()),
                source: Some(source.as_str().to_string()),
                typ: Some(VOLUME),
                read_only: Some(false),
                ..Mount::default()
            })
            .map(|v| (v.target.clone().unwrap().to_string(), v.clone()))
            .collect::<HashMap<_, _>>() //dedupe entries to avoid duplicate mounts
            .into_values()
            .collect::<Vec<_>>();

        Ok(mounts_v2)

        //TODO: initialize volumes according to the DataEntry type

        // TODO: NEXT STEPS
        // TODO 1: implement populating volumes with files
        // TODO 2: implement symbolic linking (entrypoint wrapping)
        // TODO 3: maybe include chowning in entrypoint wrapping

        //TODO: all built-in stuff must be included in v2 - caches, ssh, system-config, etc.

        //TODO caches and system shared volumes (ssh-key) shall maybe owned by a rooz group that need to be
        // ensured in containers and the user would beed to be added to that group to read (and write as the group - caches)

        // TODO: DESIGN CHANGES - BREAKING
        // /work is not longer backed by a volume by default
        // In volumes-v2 it can be explicitly configured for workspaces with configuration files, but
        // can't in tmp or simple persistent workspaces without config
    }

    pub async fn ensure_volume(
        &self,
        name: &str,
        force_recreate: bool,
        labels: Option<Labels>,
    ) -> Result<VolumeResult, AnyError> {
        let create_vol_options = VolumeCreateOptions {
            name: Some(name.into()),
            labels: labels.map(|x| x.into()),
            ..Default::default()
        };

        match self.client.inspect_volume(&name).await {
            Ok(_) if force_recreate => {
                let options = RemoveVolumeOptions { force: true };
                self.client.remove_volume(&name, Some(options)).await?;
                self.create_volume(create_vol_options).await
            }
            Ok(_) => {
                log::debug!("Reusing an existing {} volume", &name);
                Ok(VolumeResult::AlreadyExists)
            }
            Err(DockerResponseServerError {
                status_code: 404,
                message: _,
            }) => self.create_volume(create_vol_options).await,
            Err(e) => panic!("{}", e),
        }
    }

    pub async fn ensure_mounts(
        &self,
        volumes: &Vec<RoozVolume>,
        tilde_replacement: Option<&str>,
        uid: Option<&str>,
    ) -> Result<Vec<Mount>, AnyError> {
        let mut mounts = vec![];
        for v in volumes {
            let mount = self
                .ensure_mount(&v, tilde_replacement, v.labels.clone())
                .await?;
            if let RoozVolume {
                path,
                files: Some(files),
                ..
            } = v
            {
                self.ensure_file(&v.safe_volume_name(), path, &files, mount.clone(), uid)
                    .await?
            };

            mounts.push(mount);
        }
        Ok(mounts.clone())
    }

    async fn ensure_mount(
        &self,
        volume: &RoozVolume,
        tilde_replacement: Option<&str>,
        labels: Option<Labels>,
    ) -> Result<Mount, AnyError> {
        log::debug!("Process volume: {:?}", &volume);
        let mount = volume.to_mount(tilde_replacement);
        if let Some(name) = &mount.source {
            self.ensure_volume(&name, false, labels).await?;
        }
        Ok(mount)
    }

    async fn ensure_file(
        &self,
        volume_name: &str,
        path: &str,
        files: &Vec<RoozVolumeFile>,
        mount: Mount,
        uid: Option<&str>,
    ) -> Result<(), AnyError> {
        let mut cmd = files
            .iter()
            .map(|f| {
                let p = Path::new(path)
                    .join(&f.file_path)
                    .to_string_lossy()
                    .to_string();
                format!(
                    "echo '{}' | base64 -d > {}",
                    general_purpose::STANDARD.encode(f.data.trim()),
                    p,
                )
            })
            .collect::<Vec<_>>()
            .join(" && ".into());

        if let Some(uid) = uid {
            let chown = format!("chown -R {}:{} {} && ", uid, uid, path);
            cmd = format!("{}{}", chown, cmd)
        }

        self.container
            .one_shot(
                &format!("populate volume: {}", &volume_name),
                cmd,
                Some(vec![mount]),
                None,
                None,
            )
            .await?;

        Ok(())
    }
}
