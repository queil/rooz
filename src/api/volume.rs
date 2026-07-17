use std::collections::{HashMap, HashSet};

use crate::config::config::{DataEntry, DataExt, DataValue, MountSource};
use crate::model::types::{
    ContentGenerator, DataEntryKey, DataEntryVolumeSpec, FileName, FileSpec, OneShotResult,
    TargetDir, TargetPath, UserFile, VolumeFilesSpec, VolumeName, VolumeSpec,
};
use crate::util::id;
use crate::util::labels::DATA_ROLE;
use crate::{
    api::VolumeApi,
    constants,
    model::{
        types::{AnyError, VolumeResult},
        volume::{RoozVolume, VolumeFile},
    },
    util::labels::Labels,
};
use bollard::{
    errors::Error::DockerResponseServerError,
    models::{MountVolumeOptions, Volume},
    query_parameters::{ListVolumesOptions, RemoveVolumeOptions},
    service::Mount,
};
use bollard_stubs::models::MountType::VOLUME;
use bollard_stubs::models::VolumeCreateRequest;

// where the volume gets mounted inside the populate one-shot container
const POPULATE_DIR: &str = "/var/lib/rooz";

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

    pub async fn ensure_volume(&self, spec: &VolumeSpec) -> Result<VolumeResult, AnyError> {
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

    pub fn real_mounts(
        mounts: HashMap<TargetPath, DataEntryVolumeSpec>,
        home_dir: Option<&str>,
    ) -> HashMap<TargetDir, VolumeFilesSpec> {
        mounts
            .iter()
            .map(|(target, source_entry)| {
                let expanded_target = Self::expand_home(target.as_str().to_string(), home_dir);
                let (real_target, maybe_file) = match source_entry.data.clone() {
                    DataEntry::File {
                        generator,
                        executable,
                        ..
                    } => (
                        expanded_target.clone(),
                        Some(FileSpec {
                            file_name: FileName(source_entry.data.clone().name()),
                            user_file: UserFile(expanded_target),
                            generator,
                            executable,
                        }),
                    ),
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
    pub async fn ensure_volumes(
        &self,
        data_entries: &HashMap<DataEntryKey, DataEntryVolumeSpec>,
    ) -> Result<HashMap<VolumeName, VolumeResult>, AnyError> {
        let mut result = HashMap::new();
        for (k, v) in data_entries
            .iter()
            .map(|(_, v)| (v.volume.name.clone(), v.volume.clone()))
            .collect::<HashMap<_, _>>()
        {
            let volume_result = self.ensure_volume(&v).await?;
            result.insert(VolumeName(k), volume_result);
        }
        Ok(result)
    }

    pub async fn populate_volume(
        &self,
        volume_file: VolumeFilesSpec,
        uid: Option<i32>,
    ) -> Result<(), AnyError> {
        let populate_target = TargetDir(POPULATE_DIR.to_string());
        self.ensure_files(
            POPULATE_DIR,
            &volume_file.clone(),
            Self::populate_mount(&populate_target, &volume_file),
            uid,
        )
        .await
    }

    pub fn mount(target: &TargetDir, source: &VolumeFilesSpec) -> Mount {
        debug_assert!(
            source.files.len() <= 1,
            "user-container mount expects 0 or 1 file per target; got {} for {}",
            source.files.len(),
            target.as_str()
        );

        let subpath = source
            .files
            .first()
            .map(|f| f.file_name.as_str().to_string());

        Mount {
            target: Some(target.as_str().to_string()),
            source: Some(source.volume_name.as_str().to_string()),
            typ: Some(VOLUME),
            read_only: Some(false),
            volume_options: subpath.map(|sp| MountVolumeOptions {
                subpath: Some(sp),
                ..Default::default()
            }),
            ..Mount::default()
        }
    }

    pub fn populate_mount(target: &TargetDir, source: &VolumeFilesSpec) -> Mount {
        Mount {
            target: Some(target.as_str().to_string()),
            source: Some(source.volume_name.as_str().to_string()),
            typ: Some(VOLUME),
            read_only: Some(false),
            ..Mount::default()
        }
    }

    pub async fn mounts(
        &self,
        real_mounts: &HashMap<TargetDir, VolumeFilesSpec>,
    ) -> Result<Vec<Mount>, AnyError> {
        let mut mount_entries = HashMap::new();
        mount_entries.extend(real_mounts.clone());

        let mounts = mount_entries
            .into_iter()
            .map(|(target, source)| Self::mount(&target, &source))
            .map(|v| (v.target.clone().unwrap().to_string(), v.clone()))
            .collect::<HashMap<_, _>>() //dedupe entries to avoid duplicate mounts
            .into_values()
            .collect::<Vec<_>>();

        Ok(mounts)
    }

    pub async fn ensure_mounts(
        &self,
        volumes: &Vec<RoozVolume>,
        tilde_replacement: Option<&str>,
    ) -> Result<Vec<Mount>, AnyError> {
        let mut mounts = vec![];
        for v in volumes {
            log::debug!("Process volume: {:?}", &v);
            self.ensure_volume(&v.to_spec()).await?;
            mounts.push(v.to_mount(tilde_replacement));
        }
        Ok(mounts)
    }

    pub async fn write_files(
        &self,
        volume: &RoozVolume,
        files: &[VolumeFile],
        uid: Option<i32>,
    ) -> Result<(), AnyError> {
        let spec = volume.to_spec();
        self.ensure_volume(&spec).await?;
        self.populate(&spec.name, &volume.path, files, volume.to_mount(None), uid)
            .await
    }

    fn files_tar(files: &[VolumeFile], uid: Option<i32>) -> Result<Vec<u8>, AnyError> {
        let mut builder = tar::Builder::new(Vec::new());
        for f in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(f.content.len() as u64);
            header.set_mode(if f.executable { 0o755 } else { 0o644 });
            let uid = uid.unwrap_or(constants::ROOT_UID_INT) as u64;
            header.set_uid(uid);
            header.set_gid(uid);
            builder.append_data(&mut header, &f.path, f.content.as_bytes())?;
        }
        Ok(builder.into_inner()?)
    }

    async fn populate(
        &self,
        volume_name: &str,
        root_dir: &str,
        files: &[VolumeFile],
        mount: Mount,
        uid: Option<i32>,
    ) -> Result<(), AnyError> {
        let tar = if files.is_empty() {
            None
        } else {
            Some(Self::files_tar(files, uid)?)
        };

        // the tar carries file ownership already but the volume root dir
        // still needs chowning so the workspace user can write to it
        let chown = match uid {
            Some(uid) if uid != constants::ROOT_UID_INT => {
                Some(format!("chown -R {}:{} {}", uid, uid, root_dir))
            }
            _ => None,
        };

        if tar.is_none() && chown.is_none() {
            return Ok(());
        }

        self.container
            .one_shot_upload(
                &format!("populate volume: {}", volume_name),
                vec![mount],
                root_dir,
                tar,
                chown,
            )
            .await
    }

    async fn ensure_files(
        &self,
        root_dir: &str,
        spec: &VolumeFilesSpec,
        mount: Mount,
        uid: Option<i32>,
    ) -> Result<(), AnyError> {
        let mut tar_files = Vec::new();
        for f in &spec.files {
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

            tar_files.push(VolumeFile {
                path: f.file_name.as_str().to_string(),
                // IMPORTANT: never trim content so YAML multi-line strings are respected and can
                // control whitespace and most importantly EOLs
                content,
                executable: f.executable,
            });
        }

        self.populate(spec.volume_name.as_str(), root_dir, &tar_files, mount, uid)
            .await
    }
}

#[cfg(test)]
mod tests {
    use crate::api::VolumeApi;
    use crate::config::config::{DataEntry, DataValue, MountSource};
    use crate::model::types::{
        ContentGenerator, DataEntryKey, DataEntryVolumeSpec, FileName, FileSpec, TargetDir,
        TargetPath, UserFile, VolumeFilesSpec, VolumeName, VolumeSpec,
    };
    use crate::model::volume::VolumeFile;
    use std::collections::HashMap;

    fn dir() -> DataValue {
        DataValue::Dir {}
    }

    fn inline(content: &str) -> DataValue {
        DataValue::InlineContent {
            content: content.to_string(),
            executable: None,
        }
    }

    #[test]
    fn implicit_work_added_when_no_mounts() {
        let specs = VolumeApi::create_volume_specs("ws", &HashMap::new(), &HashMap::new(), true);
        assert_eq!(specs.len(), 1);
        let work = specs.get(&DataEntryKey("work".to_string())).unwrap();
        assert_eq!(work.volume.name, "rooz-ws-work");
    }

    #[test]
    fn no_implicit_work_empty_result() {
        let specs = VolumeApi::create_volume_specs("ws", &HashMap::new(), &HashMap::new(), false);
        assert!(specs.is_empty());
    }

    #[test]
    fn dir_entry_via_reference_mount() {
        let mut data = HashMap::new();
        data.insert("mydir".to_string(), dir());

        let mut mounts = HashMap::new();
        mounts.insert(
            ("work".to_string(), "/mydir".to_string()),
            MountSource::DataEntryReference(DataEntryKey("mydir".to_string())),
        );

        let specs = VolumeApi::create_volume_specs("ws", &data, &mounts, false);
        let entry = specs.get(&DataEntryKey("mydir".to_string())).unwrap();
        assert_eq!(entry.volume.name, "rooz-ws-mydir");
    }

    #[test]
    fn inline_file_mount_gets_inline_fallback_volume() {
        let mut mounts = HashMap::new();
        mounts.insert(
            ("work".to_string(), "/config".to_string()),
            MountSource::InlineDataValue(inline("hello")),
        );

        let specs = VolumeApi::create_volume_specs("ws", &HashMap::new(), &mounts, false);
        // key = sanitize("/config") = "-config"
        let entry = specs.get(&DataEntryKey("-config".to_string())).unwrap();
        assert_eq!(entry.volume.name, "rooz-ws-inline");
    }

    #[test]
    fn multiple_inline_files_share_inline_volume() {
        let mut mounts = HashMap::new();
        mounts.insert(
            ("work".to_string(), "/file-a".to_string()),
            MountSource::InlineDataValue(inline("aaa")),
        );
        mounts.insert(
            ("work".to_string(), "/file-b".to_string()),
            MountSource::InlineDataValue(inline("bbb")),
        );

        let specs = VolumeApi::create_volume_specs("ws", &HashMap::new(), &mounts, false);
        assert_eq!(specs.len(), 2);
        let a = specs.get(&DataEntryKey("-file-a".to_string())).unwrap();
        let b = specs.get(&DataEntryKey("-file-b".to_string())).unwrap();
        assert_eq!(a.volume.name, "rooz-ws-inline");
        assert_eq!(b.volume.name, "rooz-ws-inline");
    }

    #[test]
    fn real_mounts_dir_has_no_files() {
        let mut mounts = HashMap::new();
        mounts.insert(
            TargetPath("/work".to_string()),
            DataEntryVolumeSpec {
                data: DataEntry::Dir {
                    name: "work".to_string(),
                },
                volume: VolumeSpec {
                    name: "rooz-ws-work".to_string(),
                    labels: None,
                },
            },
        );

        let real = VolumeApi::real_mounts(mounts, None);
        let entry = real.get(&TargetDir("/work".to_string())).unwrap();
        assert_eq!(entry.volume_name.as_str(), "rooz-ws-work");
        assert!(entry.files.is_empty());
    }

    #[test]
    fn real_mounts_file_has_shadow_path_and_user_path() {
        let mut mounts = HashMap::new();
        mounts.insert(
            TargetPath("~/.myconfig".to_string()),
            DataEntryVolumeSpec {
                data: DataEntry::File {
                    name: "myconfig".to_string(),
                    generator: ContentGenerator::Inline("content".to_string()),
                    executable: false,
                },
                volume: VolumeSpec {
                    name: "rooz-ws-inline".to_string(),
                    labels: None,
                },
            },
        );

        let real = VolumeApi::real_mounts(mounts, Some("/home/user"));
        let entry = real
            .get(&TargetDir("/home/user/.myconfig".to_string()))
            .unwrap();
        assert_eq!(entry.volume_name.as_str(), "rooz-ws-inline");
        assert_eq!(entry.files.len(), 1);
        let f = &entry.files[0];
        assert_eq!(f.user_file.as_str(), "/home/user/.myconfig");
        assert_eq!(f.file_name.as_str(), "myconfig");
    }

    #[test]
    fn real_mounts_no_home_keeps_tilde() {
        let mut mounts = HashMap::new();
        mounts.insert(
            TargetPath("~/.myconfig".to_string()),
            DataEntryVolumeSpec {
                data: DataEntry::File {
                    name: "myconfig".to_string(),
                    generator: ContentGenerator::Inline("content".to_string()),
                    executable: false,
                },
                volume: VolumeSpec {
                    name: "rooz-ws-inline".to_string(),
                    labels: None,
                },
            },
        );

        let real = VolumeApi::real_mounts(mounts, None);
        assert!(
            real.contains_key(&TargetDir("~/.myconfig".to_string())),
            "without a home dir the tilde must be left as-is"
        );
    }

    #[test]
    fn real_mounts_file_keeps_entry_name() {
        // files keep the entry name verbatim so entries like
        // 'app.yaml' and 'app.json' cannot collide
        let mut mounts = HashMap::new();
        mounts.insert(
            TargetPath("/etc/app.yaml".to_string()),
            DataEntryVolumeSpec {
                data: DataEntry::File {
                    name: "app.yaml".to_string(),
                    generator: ContentGenerator::Inline("content".to_string()),
                    executable: false,
                },
                volume: VolumeSpec {
                    name: "rooz-ws-inline".to_string(),
                    labels: None,
                },
            },
        );

        let real = VolumeApi::real_mounts(mounts, None);
        let entry = real.get(&TargetDir("/etc/app.yaml".to_string())).unwrap();
        assert_eq!(entry.files[0].file_name.as_str(), "app.yaml");
    }

    #[test]
    fn real_mounts_same_volume_distinct_targets() {
        let spec = |name: &str| DataEntryVolumeSpec {
            data: DataEntry::File {
                name: name.to_string(),
                generator: ContentGenerator::Inline("content".to_string()),
                executable: false,
            },
            volume: VolumeSpec {
                name: "rooz-ws-inline".to_string(),
                labels: None,
            },
        };
        let mut mounts = HashMap::new();
        mounts.insert(TargetPath("/etc/file-a".to_string()), spec("file-a"));
        mounts.insert(TargetPath("/etc/file-b".to_string()), spec("file-b"));

        let real = VolumeApi::real_mounts(mounts, None);
        assert_eq!(real.len(), 2);
        for entry in real.values() {
            assert_eq!(entry.volume_name.as_str(), "rooz-ws-inline");
            assert_eq!(entry.files.len(), 1);
        }
    }

    #[test]
    fn spec_carries_generator_and_executable() {
        let mut data = HashMap::new();
        data.insert(
            "gen".to_string(),
            DataValue::GeneratedContent {
                generate: "echo hi".to_string(),
                image: Some("alpine:latest".to_string()),
                executable: Some(true),
            },
        );

        let mut mounts = HashMap::new();
        mounts.insert(
            ("work".to_string(), "/gen".to_string()),
            MountSource::DataEntryReference(DataEntryKey("gen".to_string())),
        );

        let specs = VolumeApi::create_volume_specs("ws", &data, &mounts, false);
        let entry = specs.get(&DataEntryKey("gen".to_string())).unwrap();
        match &entry.data {
            DataEntry::File {
                generator: ContentGenerator::Script { script, image },
                executable,
                ..
            } => {
                assert_eq!(script, "echo hi");
                assert_eq!(image.as_deref(), Some("alpine:latest"));
                assert!(executable);
            }
            other => panic!("expected a script-generated file entry, got {:?}", other),
        }
    }

    #[test]
    fn mount_with_file_sets_subpath() {
        let spec = VolumeFilesSpec {
            volume_name: VolumeName("rooz-ws-inline".to_string()),
            files: vec![FileSpec {
                file_name: FileName("myconfig".to_string()),
                user_file: UserFile("/home/user/.myconfig".to_string()),
                generator: ContentGenerator::Inline("x".to_string()),
                executable: false,
            }],
        };
        let target = TargetDir("/home/user/.myconfig".to_string());

        let m = VolumeApi::mount(&target, &spec);
        assert_eq!(m.target.as_deref(), Some("/home/user/.myconfig"));
        assert_eq!(m.source.as_deref(), Some("rooz-ws-inline"));
        let subpath = m.volume_options.expect("volume options expected").subpath;
        assert_eq!(subpath.as_deref(), Some("myconfig"));
    }

    #[test]
    fn mount_without_files_has_no_subpath() {
        let spec = VolumeFilesSpec {
            volume_name: VolumeName("rooz-ws-work".to_string()),
            files: vec![],
        };
        let target = TargetDir("/work".to_string());

        let m = VolumeApi::mount(&target, &spec);
        assert!(m.volume_options.is_none());
    }

    fn untar(bytes: &[u8]) -> Vec<(String, u32, u64, u64, String)> {
        use std::io::Read;
        let mut archive = tar::Archive::new(bytes);
        archive
            .entries()
            .unwrap()
            .map(|e| {
                let mut e = e.unwrap();
                let path = e.path().unwrap().to_string_lossy().into_owned();
                let header = e.header();
                let (mode, uid, gid) = (
                    header.mode().unwrap(),
                    header.uid().unwrap(),
                    header.gid().unwrap(),
                );
                let mut content = String::new();
                e.read_to_string(&mut content).unwrap();
                (path, mode, uid, gid, content)
            })
            .collect()
    }

    #[test]
    fn files_tar_roundtrip() {
        let files = vec![
            VolumeFile {
                path: "plain.data".to_string(),
                content: "line1\n\nline3\n".to_string(),
                executable: false,
            },
            VolumeFile {
                path: "script.data".to_string(),
                content: "#!/bin/sh\necho hi\n".to_string(),
                executable: true,
            },
        ];

        let bytes = VolumeApi::files_tar(&files, Some(1000)).unwrap();
        let entries = untar(&bytes);

        assert_eq!(entries.len(), 2);
        let (path, mode, uid, gid, content) = &entries[0];
        assert_eq!(path, "plain.data");
        assert_eq!(*mode, 0o644);
        assert_eq!((*uid, *gid), (1000, 1000));
        assert_eq!(content, "line1\n\nline3\n");

        let (path, mode, _, _, content) = &entries[1];
        assert_eq!(path, "script.data");
        assert_eq!(*mode, 0o755);
        assert_eq!(content, "#!/bin/sh\necho hi\n");
    }

    #[test]
    fn files_tar_defaults_to_root_ownership() {
        let files = vec![VolumeFile {
            path: "cfg".to_string(),
            content: "x".to_string(),
            executable: false,
        }];

        let bytes = VolumeApi::files_tar(&files, None).unwrap();
        let (_, _, uid, gid, _) = &untar(&bytes)[0];
        assert_eq!((*uid, *gid), (0, 0));
    }

    #[test]
    fn populate_mount_mounts_volume_root() {
        let spec = VolumeFilesSpec {
            volume_name: VolumeName("rooz-ws-inline".to_string()),
            files: vec![FileSpec {
                file_name: FileName("myconfig".to_string()),
                user_file: UserFile("/home/user/.myconfig".to_string()),
                generator: ContentGenerator::Inline("x".to_string()),
                executable: false,
            }],
        };
        let target = TargetDir("/var/lib/rooz".to_string());

        let m = VolumeApi::populate_mount(&target, &spec);
        assert_eq!(m.target.as_deref(), Some("/var/lib/rooz"));
        assert!(m.volume_options.is_none());
    }
}
