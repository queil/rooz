use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::config::config::{DataEntry, DataExt, DataValue, MountSource};
use crate::model::types::{
    ContentGenerator, DataEntryKey, DataEntryVolumeSpec, FileSpec, OneShotResult, TargetDir,
    TargetFile, TargetPath, UserFile, VolumeFilesSpec, VolumeName, VolumeSpec,
};
use crate::util::id;
use crate::util::labels::DATA_ROLE;
use crate::{
    api::VolumeApi,
    constants,
    model::{
        types::{AnyError, VolumeResult},
        volume::{RoozVolume, RoozVolumeFile},
    },
    util::labels::Labels,
};
use base64::{Engine as _, engine::general_purpose};
use bollard::{
    errors::Error::DockerResponseServerError,
    models::Volume,
    query_parameters::{ListVolumesOptions, RemoveVolumeOptions},
    service::Mount,
};
use bollard_stubs::models::MountTypeEnum::VOLUME;
use bollard_stubs::models::VolumeCreateRequest;

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

    async fn create_volume(&self, options: VolumeCreateRequest) -> Result<VolumeResult, AnyError> {
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
                self.create_volume(VolumeCreateRequest {
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
        implicit_work: bool,
    ) -> HashMap<TargetPath, DataEntryVolumeSpec> {
        let mut result = HashMap::new();

        let mut mount_entries: HashMap<String, String> = HashMap::new();
        mount_entries.extend(mounts.clone());

        if !mounts.values().any(|key| key == "work") && implicit_work {
            mount_entries.insert("/work".to_string(), "work".to_string());
        }

        for (target, source_key) in mount_entries {
            let source_exists = &volumes.contains_key(&DataEntryKey(source_key.to_string()));
            if !source_exists {
                panic!(
                    "Key '{}' not found under 'data:' in workspace config. Keys: {:?}",
                    source_key.as_str(),
                    &volumes.keys(),
                );
            }

            result.insert(
                TargetPath(target.clone()),
                volumes[&DataEntryKey(source_key)].clone(),
            );
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
        mounts: &HashMap<(String, String), MountSource>,
        implicit_work: bool,
    ) -> HashMap<DataEntryKey, DataEntryVolumeSpec> {
        let data = &mounts
            .iter()
            .map(|((_, target_path), v)| match v {
                MountSource::DataEntryReference(data_key) => (data_key.as_str().to_string(), {
                    let source_exists = data.contains_key(data_key.as_str());
                    if !source_exists {
                        panic!(
                            "Key '{}' not found under 'data:' in workspace config. Keys: {:?}",
                            data_key.as_str(),
                            &data.keys(),
                        );
                    }

                    data[data_key.as_str()].clone()
                }),
                MountSource::InlineDataValue(data_value) => {
                    (id::sanitize(target_path), data_value.to_owned())
                }
            })
            .collect::<HashMap<String, DataValue>>();

        let mut data_entries = vec![];
        data_entries.extend_from_slice(data.clone().into_entries().as_slice());

        if !data.contains_key("work") && implicit_work {
            data_entries.push(DataEntry::Dir {
                name: "work".to_string(),
            });
        }

        let ref_mounts = mounts
            .into_iter()
            .filter_map(|((container_name, _), v)| match v {
                MountSource::DataEntryReference(data_key) => Some((data_key, container_name)),
                MountSource::InlineDataValue(_) => None,
            })
            .fold(
                HashMap::<&DataEntryKey, HashSet<&String>>::new(),
                |mut acc, (data_key, container_name)| {
                    acc.entry(data_key).or_default().insert(container_name);
                    acc
                },
            )
            .iter()
            .map(|(data_key, _)| {
                (
                    *data_key,
                    VolumeSpec {
                        name: Self::volume_name(workspace_key, data_key.as_str()),
                        labels: Some(Labels::from(&[
                            Labels::workspace(workspace_key),
                            Labels::role(DATA_ROLE),
                        ])),
                    },
                )
            })
            .collect::<HashMap<_, _>>();

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
                DataEntry::File { name, .. } => {
                    let volume_spec = ref_mounts
                        .get(&DataEntryKey(name.to_string()))
                        .map(|z| z.clone())
                        .unwrap_or(VolumeSpec {
                            name: Self::volume_name(workspace_key, "inline"),
                            labels: Some(Labels::from(&[
                                Labels::workspace(workspace_key),
                                Labels::role(DATA_ROLE),
                            ])),
                        });

                    Some((
                        DataEntryKey(name.to_string()),
                        DataEntryVolumeSpec {
                            data: d.clone(),
                            volume: volume_spec,
                        },
                    ))
                }
            })
            .collect::<HashMap<_, _>>()
    }

    pub fn real_mounts_v2(
        mounts: HashMap<TargetPath, DataEntryVolumeSpec>,
        home_dir: Option<&str>,
    ) -> HashMap<TargetDir, VolumeFilesSpec> {
        const SHADOW_ROOT_DIR: &str = "/var/lib/rooz";
        mounts
            .iter()
            .map(|(target, source_entry)| {
                let expanded_target = Self::expand_home(target.as_str().to_string(), home_dir);
                let (real_target, maybe_file) = match source_entry.data.clone() {
                    DataEntry::File {
                        generator,
                        executable,
                        ..
                    } => {
                        let shadow_subpath = &source_entry.clone().data.name();
                        let shadow_file = Path::new(SHADOW_ROOT_DIR).join(shadow_subpath).join(
                            Path::new(&expanded_target)
                                .with_file_name(shadow_subpath)
                                .with_extension("data")
                                .to_string_lossy()
                                .trim_start_matches('/'),
                        );

                        (
                            format!("{}/{}", SHADOW_ROOT_DIR.to_string(), shadow_subpath),
                            Some(FileSpec {
                                target_file: TargetFile(shadow_file.to_string_lossy().into_owned()),
                                user_file: UserFile(expanded_target),
                                generator,
                                executable,
                            }),
                        )
                    }
                    _ => (expanded_target, None),
                };
                (
                    TargetDir(real_target),
                    VolumeName(source_entry.volume.name.to_string()),
                    maybe_file,
                )
            })
            .fold(
                HashMap::new(),
                |mut acc, (target_dir, volume_name, maybe_file)| {
                    acc.entry(target_dir)
                        .or_insert_with(|| VolumeFilesSpec {
                            volume_name,
                            files: Vec::new(),
                        })
                        .files
                        .extend(maybe_file);
                    acc
                },
            )
    }
    pub async fn ensure_volumes_v2(
        &self,
        data_entries: &HashMap<DataEntryKey, DataEntryVolumeSpec>,
    ) -> Result<HashMap<VolumeName, VolumeResult>, AnyError> {
        let mut result = HashMap::new();
        for (k, v) in data_entries
            .iter()
            .map(|(_, v)| (v.volume.name.clone(), v.volume.clone()))
            .collect::<HashMap<_, _>>()
        {
            let volume_result = self.ensure_volume_v2(&v).await?;
            result.insert(VolumeName(k), volume_result);
        }
        Ok(result)
    }

    pub async fn populate_volume(
        &self,
        target_dir: TargetDir,
        volume_file: VolumeFilesSpec,
        uid: Option<i32>,
    ) -> Result<(), AnyError> {
        self.ensure_file_v2(
            target_dir.as_str(),
            &volume_file.clone(),
            Self::mount(&target_dir, &volume_file),
            uid,
        )
        .await
    }

    pub fn mount(target: &TargetDir, source: &VolumeFilesSpec) -> Mount {
        Mount {
            target: Some(target.as_str().to_string()),
            source: Some(source.volume_name.as_str().to_string()),
            typ: Some(VOLUME),
            read_only: Some(false),
            ..Mount::default()
        }
    }
    pub async fn mounts_v2(
        &self,
        real_mounts: &HashMap<TargetDir, VolumeFilesSpec>,
    ) -> Result<Vec<Mount>, AnyError> {
        let mut mount_entries = HashMap::new();
        mount_entries.extend(real_mounts.clone());

        let mounts_v2 = mount_entries
            .into_iter()
            .map(|(target, source)| Self::mount(&target, &source))
            .map(|v| (v.target.clone().unwrap().to_string(), v.clone()))
            .collect::<HashMap<_, _>>() //dedupe entries to avoid duplicate mounts
            .into_values()
            .collect::<Vec<_>>();

        Ok(mounts_v2)
    }

    pub async fn ensure_volume(
        &self,
        name: &str,
        force_recreate: bool,
        labels: Option<Labels>,
    ) -> Result<VolumeResult, AnyError> {
        let create_vol_options = VolumeCreateRequest {
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

    async fn ensure_file_v2(
        &self,
        root_dir: &str,
        spec: &VolumeFilesSpec,
        mount: Mount,
        uid: Option<i32>,
    ) -> Result<(), AnyError> {
        let mut cmds = Vec::new();
        for f in &spec.files {
            let parent_dir = Path::new(f.target_file.as_str())
                .parent()
                .unwrap()
                .to_string_lossy()
                .into_owned();

            let content = match &f.generator {
                ContentGenerator::Inline(content) => content.to_string(),
                ContentGenerator::Script { script, image } => {
                    match self
                        .container
                        .one_shot_output(
                            &format!("generate file: {}", f.user_file.as_str()),
                            script.to_string(),
                            None,
                            None,
                            image.as_deref(),
                        )
                        .await
                    {
                        Ok(OneShotResult { data }) => data,
                        Err(e) => {
                            return Err(format!(
                                "Failed generating file: {}\n{}",
                                f.user_file.as_str(),
                                e
                            )
                            .into());
                        }
                    }
                }
            };

            cmds.push(format!(
                "mkdir -p {} && echo '{}' | base64 -d > {}{}",
                parent_dir,
                general_purpose::STANDARD.encode(content.trim()),
                f.target_file.as_str(),
                if f.executable {
                    format!(" && chmod +x {}", f.target_file.as_str())
                } else {
                    "".to_string()
                }
            ));
        }

        let mut cmd = cmds.join(" && ".into());

        if let Some(uid) = uid
            && uid != constants::ROOT_UID_INT
        {
            let chown = format!("chown -R {}:{} {}", uid, uid, root_dir);
            cmd = format!(
                "{}{}{}",
                cmd,
                if cmd.is_empty() { "" } else { " && " },
                chown
            )
        }

        self.container
            .one_shot(
                &format!("populate volume: {}", &spec.volume_name.as_str()),
                cmd,
                Some(vec![mount]),
                None,
                None,
            )
            .await?;

        Ok(())
    }
    async fn ensure_file(
        &self,
        volume_name: &str,
        parent_dir: &str,
        files: &Vec<RoozVolumeFile>,
        mount: Mount,
        uid: Option<&str>,
    ) -> Result<(), AnyError> {
        let mut cmd = files
            .iter()
            .map(|f| {
                let p = Path::new(parent_dir)
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
            let chown = format!("chown -R {}:{} {}", uid, uid, parent_dir);
            cmd = format!(
                "{}{}{}",
                cmd,
                if cmd.is_empty() { "" } else { " && " },
                chown
            )
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
