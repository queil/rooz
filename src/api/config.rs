use std::io;

use crate::{
    config::config::{
        ConfigType, DataValue, DirFile, DirFilter, FileFormat, MAX_DATA_FILE_BYTES, MAX_DIR_BYTES,
        MAX_DIR_FILES, RoozCfg, SystemConfig, validate_relative_config_path,
        validate_relative_file_path,
    },
    model::{
        types::AnyError,
        volume::{RoozVolume, RoozVolumeRole, VolumeFile},
    },
    util::{labels::Labels, sh},
};

use age::x25519::Identity;
use colored::Colorize;

use super::{Api, ConfigApi};

pub struct ConfigBody {
    pub body: String,
    pub bases: Option<String>,
    pub merged: Option<RoozCfg>,
}

#[async_trait::async_trait]
pub trait ConfigReader {
    async fn read_file(&self, path: &str) -> Result<String, AnyError>;

    async fn read_data_file(&self, dir: &str, rel: &str) -> Result<String, AnyError>;

    // every file below `rel` the filter keeps, with paths relative to it
    async fn read_data_dir(
        &self,
        dir: &str,
        rel: &str,
        filter: &DirFilter,
    ) -> Result<Vec<DirFile>, AnyError>;
}

fn refused(rel: &str, why: &str) -> AnyError {
    format!("'{}' {}", rel, why).into()
}

// A dir's files are checked one by one like a `file:`'s, and together against the dir's limits
fn check_dir_listing(rel: &str, listing: &[(String, u64)]) -> Result<(), AnyError> {
    if listing.len() > MAX_DIR_FILES {
        return Err(refused(
            rel,
            &format!("has more than {} files", MAX_DIR_FILES),
        ));
    }
    if listing.iter().map(|(_, size)| size).sum::<u64>() > MAX_DIR_BYTES as u64 {
        return Err(refused(
            rel,
            &format!(
                "holds more than {} KiB - 'dir:' is for small files like dotfiles",
                MAX_DIR_BYTES / 1024
            ),
        ));
    }
    for (path, size) in listing {
        let full = format!("{}/{}", rel, path);
        validate_relative_file_path(&full)?;
        if *size > MAX_DATA_FILE_BYTES as u64 {
            return Err(size_refusal(&full));
        }
    }
    Ok(())
}

fn size_refusal(rel: &str) -> AnyError {
    refused(
        rel,
        &format!(
            "is larger than {} KiB - 'file:' is for small files like scripts and dotfiles",
            MAX_DATA_FILE_BYTES / 1024
        ),
    )
}

pub async fn resolve_data_files<R: ConfigReader + Sync>(
    reader: &R,
    cfg_path: &str,
    cfg: &mut RoozCfg,
) -> Result<(), AnyError> {
    let dir = std::path::Path::new(cfg_path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    for source in cfg.dir_sources_mut() {
        validate_relative_file_path(&source.dir)?;
        let filter = DirFilter::new(source)
            .map_err(|e| format!("Data dir {} (declared in {})", e, cfg_path))?;
        source.files = Some(
            reader
                .read_data_dir(&dir, &source.dir, &filter)
                .await
                .map_err(|e| format!("Data dir {} (declared in {})", e, cfg_path))?,
        );
    }
    for value in cfg.data_values_mut() {
        if let DataValue::FileContent { file, content, .. } = value {
            validate_relative_file_path(file)?;
            *content = Some(
                reader
                    .read_data_file(&dir, file)
                    .await
                    .map_err(|e| format!("Data file {} (declared in {})", e, cfg_path))?,
            );
        }
    }
    Ok(())
}

pub fn render_bases(
    bases: &[(String, RoozCfg)],
    fmt: FileFormat,
) -> Result<Option<String>, AnyError> {
    let rendered = bases
        .iter()
        .map(|(path, b)| b.to_string(fmt).map(|yaml| format!("# {}\n{}", path, yaml)))
        .collect::<Result<Vec<_>, _>>()?
        .join("\n---\n");
    Ok(if rendered.is_empty() {
        None
    } else {
        Some(rendered)
    })
}

impl<'a> ConfigApi<'a> {
    async fn store_config_file(
        &self,
        workspace_key: &str,
        config_type: &ConfigType,
        content: &str,
        labels: Option<Labels>,
    ) -> Result<(), AnyError> {
        let config_vol = RoozVolume::config_data(
            workspace_key,
            "/etc/rooz",
            labels,
            Some(RoozVolumeRole::WorkspaceConfig),
        );
        self.api
            .volume
            .write_files(
                &config_vol,
                &[VolumeFile::new_private(config_type.file_path(), content)],
                None,
            )
            .await
    }

    pub async fn store(
        &self,
        workspace_key: &str,
        origin: &str,
        body: &str,
    ) -> Result<(), AnyError> {
        self.store_config_file(
            workspace_key,
            &ConfigType::Body,
            body,
            Some(Labels::from(&[Labels::config_origin(origin)])),
        )
        .await
    }

    pub async fn store_bases(&self, workspace_key: &str, body: &str) -> Result<(), AnyError> {
        self.store_config_file(workspace_key, &ConfigType::Bases, body, None)
            .await
    }

    pub async fn read(
        &self,
        workspace_key: &str,
        config_type: &ConfigType,
    ) -> Result<String, AnyError> {
        let config_path = config_type.file_path();
        let result = &self
            .api
            .container
            .one_shot_output(
                "read-workspace-config",
                format!(
                    "ls /etc/rooz/{} > /dev/null 2>&1 && cat /etc/rooz/{} || true",
                    config_path, config_path
                ),
                Some(vec![
                    RoozVolume::workspace_config_read(workspace_key, "/etc/rooz").to_mount(None),
                ]),
                None,
                None,
            )
            .await?;
        Ok(result.data.to_string())
    }

    pub async fn store_runtime(&self, workspace_key: &str, data: &str) -> Result<(), AnyError> {
        self.store_config_file(workspace_key, &ConfigType::Runtime, data, None)
            .await
    }

    fn edit_error(&self, message: &str) -> Result<(), AnyError> {
        eprintln!("{}", "Error: Invalid configuration".bold().red());
        eprintln!("{}", message.red());
        eprintln!("Press any key to continue editing...");
        // EOF (no terminal to edit in) would otherwise relaunch the editor forever
        if io::stdin().read_line(&mut String::new())? == 0 {
            return Err(format!(
                "Invalid configuration, and no input to retry with: {}",
                message
            )
            .into());
        }
        Ok(())
    }

    pub async fn edit_string(
        &self,
        body: String,
        format: FileFormat,
        identity: &Identity,
    ) -> Result<(RoozCfg, String), AnyError> {
        let mut edited_body = body;
        let mut edited_config;
        loop {
            edited_body = match edit::edit(edited_body.clone()) {
                Ok(b) => b,
                Err(err) => {
                    self.edit_error(&err.to_string())?;
                    continue;
                }
            };
            edited_config = match RoozCfg::from_string(&edited_body, format) {
                Ok(c) => c,
                Err(err) => {
                    self.edit_error(&err.to_string())?;
                    continue;
                }
            };

            if let (Some(vars), Some(secrets)) = (&edited_config.vars, &edited_config.secrets)
                && let Some(duplicate_key) =
                    vars.keys().find(|k| secrets.contains_key(&k.to_string()))
            {
                self.edit_error(&format!(
                    "The key: '{}' can be only defined in either vars or secrets.",
                    duplicate_key
                ))?;
                continue;
            };
            break;
        }
        self.encrypt(&mut edited_config, identity).await?;
        Ok((edited_config, edited_body))
    }

    pub async fn system_edit_string(
        &self,
        body: String,
    ) -> Result<(SystemConfig, String), AnyError> {
        let mut edited_body = body;
        let edited_config;
        loop {
            edited_body = match edit::edit(edited_body.clone()) {
                Ok(b) => b,
                Err(err) => {
                    self.edit_error(&err.to_string())?;
                    continue;
                }
            };
            edited_config = match SystemConfig::from_string(&edited_body) {
                Ok(c) => c,
                Err(err) => {
                    self.edit_error(&err.to_string())?;
                    continue;
                }
            };

            break;
        }
        Ok((edited_config, edited_body))
    }

    const MAX_BASES_DEPTH: usize = 2;

    pub async fn resolve_extends_chain<R: ConfigReader + Sync>(
        &self,
        reader: &R,
        child_path: &str,
        child: RoozCfg,
        depth: usize,
    ) -> Result<(RoozCfg, Vec<(String, RoozCfg)>), AnyError> {
        let base_paths = match child.bases.as_ref() {
            Some(p) if !p.is_empty() => p.clone(),
            _ => return Ok((child, vec![])),
        };

        if depth >= Self::MAX_BASES_DEPTH {
            return Err(format!("bases nesting too deep (limit {})", Self::MAX_BASES_DEPTH).into());
        }

        RoozCfg::validate_base_list(&base_paths)?;

        let parent_dir = std::path::Path::new(child_path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut individual_bases: Vec<(String, RoozCfg)> = Vec::new();
        let mut accumulated = RoozCfg::none();
        for base_path in &base_paths {
            let abs_path = if parent_dir.is_empty() {
                base_path.to_string()
            } else {
                format!("{}/{}", parent_dir, base_path)
            };

            let base_body = reader.read_file(&abs_path).await?;
            if base_body.is_empty() {
                return Err(format!("base '{}' not found or empty", base_path).into());
            }

            let base_fmt = FileFormat::from_path(base_path)?;
            let mut base = RoozCfg::deserialize_config(&base_body, base_fmt)
                .map_err(|e| format!("Failed to parse base '{}': {}", base_path, e))?;
            resolve_data_files(reader, &abs_path, &mut base).await?;

            let (resolved, _) =
                Box::pin(self.resolve_extends_chain(reader, &abs_path, base, depth + 1)).await?;
            individual_bases.push((base_path.clone(), resolved.clone()));
            accumulated.from_config(&resolved);
        }

        accumulated.from_config(&child);
        Ok((accumulated, individual_bases))
    }

    pub async fn read_config_body(
        &self,
        container_id: &str,
        clone_dir: &str,
        file_format: FileFormat,
        path: &str,
    ) -> Result<Option<ConfigBody>, AnyError> {
        validate_relative_config_path(path, "config path")?;
        let file_path = format!("{}/{}", clone_dir, path);

        let quoted_path = sh::quote(&file_path);
        let ls_cmd = format!(
            "ls -- {} > /dev/null 2>&1 && cat -- {}",
            quoted_path, quoted_path
        );
        let body = self
            .api
            .exec
            .output(
                "rooz-cfg",
                container_id,
                None,
                Some(vec!["sh", "-c", &ls_cmd]),
            )
            .await?;

        if body.is_empty() {
            return Err(format!("Config file '{}' not found or empty", path).into());
        }

        let mut cfg = RoozCfg::deserialize_config(&body, file_format)?;
        let reader = ContainerReader {
            api: self.api,
            container_id,
        };
        resolve_data_files(&reader, &file_path, &mut cfg).await?;
        let (merged, bases) = if cfg.bases.is_none() {
            (cfg, None)
        } else {
            let (merged, individual_bases) = self
                .resolve_extends_chain(&reader, &file_path, cfg, 0)
                .await?;
            (merged, render_bases(&individual_bases, file_format)?)
        };
        Ok(Some(ConfigBody {
            body,
            bases,
            merged: Some(merged),
        }))
    }
}

#[derive(Default)]
pub struct LocalReader {
    pub read: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl ConfigReader for LocalReader {
    async fn read_file(&self, path: &str) -> Result<String, AnyError> {
        self.read.lock().unwrap().push(path.to_string());
        std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read extends '{}': {}", path, e).into())
    }

    async fn read_data_file(&self, dir: &str, rel: &str) -> Result<String, AnyError> {
        let path = no_symlinks(dir, rel)?;
        let meta = std::fs::symlink_metadata(&path)?;
        if !meta.is_file() {
            return Err(refused(rel, "is not a regular file"));
        }
        if meta.len() > MAX_DATA_FILE_BYTES as u64 {
            return Err(size_refusal(rel));
        }
        self.read
            .lock()
            .unwrap()
            .push(path.to_string_lossy().into_owned());
        let bytes = std::fs::read(&path)?;
        String::from_utf8(bytes).map_err(|_| refused(rel, "is not valid UTF-8 text"))
    }

    async fn read_data_dir(
        &self,
        dir: &str,
        rel: &str,
        filter: &DirFilter,
    ) -> Result<Vec<DirFile>, AnyError> {
        let root = no_symlinks(dir, rel)?;
        if !std::fs::symlink_metadata(&root)?.is_dir() {
            return Err(refused(rel, "is not a directory"));
        }
        let mut listing = vec![];
        Self::list_dir(&root, "", rel, filter, &mut listing)?;
        check_dir_listing(
            rel,
            &listing
                .iter()
                .map(|(p, size, _)| (p.clone(), *size))
                .collect::<Vec<_>>(),
        )?;
        let mut files = vec![];
        for (path, _, executable) in listing {
            files.push(DirFile {
                content: self
                    .read_data_file(dir, &format!("{}/{}", rel, path))
                    .await?,
                path,
                executable,
            });
        }
        Ok(files)
    }
}

impl LocalReader {
    fn list_dir(
        root: &std::path::Path,
        below: &str,
        rel: &str,
        filter: &DirFilter,
        out: &mut Vec<(String, u64, bool)>,
    ) -> Result<(), AnyError> {
        use std::os::unix::fs::PermissionsExt;
        let dir = root.join(below);
        let mut entries = std::fs::read_dir(&dir)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if below.is_empty() {
                name
            } else {
                format!("{}/{}", below, name)
            };
            let meta = std::fs::symlink_metadata(entry.path())?;
            let kind = meta.file_type();
            if kind.is_dir() {
                if !filter.skips_dir(&path) {
                    Self::list_dir(root, &path, rel, filter, out)?;
                }
                continue;
            }
            if !filter.keeps(&path) {
                continue;
            }
            if kind.is_symlink() {
                return Err(refused(
                    &format!("{}/{}", rel, path),
                    "is a symlink, which is refused",
                ));
            } else if kind.is_file() {
                out.push((path, meta.len(), meta.permissions().mode() & 0o111 != 0));
            } else {
                return Err(refused(
                    &format!("{}/{}", rel, path),
                    "is not a regular file",
                ));
            }
            if out.len() > MAX_DIR_FILES {
                return Err(refused(
                    rel,
                    &format!("has more than {} files", MAX_DIR_FILES),
                ));
            }
        }
        Ok(())
    }
}

// The path below `dir`, refused if any part of it is a symlink
fn no_symlinks(dir: &str, rel: &str) -> Result<std::path::PathBuf, AnyError> {
    let mut path = std::path::PathBuf::from(dir);
    for segment in rel.split('/') {
        path.push(segment);
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| refused(rel, &format!("cannot be read: {}", e)))?;
        if meta.file_type().is_symlink() {
            return Err(refused(
                rel,
                "is or sits behind a symlink, which is refused",
            ));
        }
    }
    Ok(path)
}

pub struct ContainerReader<'a> {
    pub api: &'a Api<'a>,
    pub container_id: &'a str,
}

#[async_trait::async_trait]
impl<'a> ConfigReader for ContainerReader<'a> {
    async fn read_file(&self, path: &str) -> Result<String, AnyError> {
        let cat_cmd = format!("cat -- {}", sh::quote(path));
        self.api
            .exec
            .output(
                "rooz-cfg-extends",
                self.container_id,
                None,
                Some(vec!["sh", "-c", &cat_cmd]),
            )
            .await
    }

    async fn read_data_file(&self, dir: &str, rel: &str) -> Result<String, AnyError> {
        let path = format!("{}/{}", dir, rel);
        let (q_path, q_dir, q_rel) = (sh::quote(&path), sh::quote(dir), sh::quote(rel));
        let check = format!(
            r#"if [ -L {p} ]; then echo "is or sits behind a symlink, which is refused"
elif [ ! -f {p} ]; then echo "is not a regular file"
elif [ "$(realpath {p} 2>/dev/null)" != "$(realpath {d} 2>/dev/null)/"{r} ]; then echo "is or sits behind a symlink, which is refused"
elif [ "$(wc -c < {p})" -gt {max} ]; then echo "too large"
else echo ok; fi"#,
            p = q_path,
            d = q_dir,
            r = q_rel,
            max = MAX_DATA_FILE_BYTES
        );
        let verdict = self
            .api
            .exec
            .output(
                "rooz-cfg-file",
                self.container_id,
                None,
                Some(vec!["sh", "-c", &check]),
            )
            .await?;
        match verdict.trim() {
            "ok" => (),
            "too large" => return Err(size_refusal(rel)),
            why => return Err(refused(rel, why)),
        }
        let cat_cmd = format!("cat -- {}", q_path);
        let content = self
            .api
            .exec
            .output(
                "rooz-cfg-file",
                self.container_id,
                None,
                Some(vec!["sh", "-c", &cat_cmd]),
            )
            .await?;
        if content.len() > MAX_DATA_FILE_BYTES {
            return Err(size_refusal(rel));
        }
        Ok(content)
    }

    async fn read_data_dir(
        &self,
        dir: &str,
        rel: &str,
        filter: &DirFilter,
    ) -> Result<Vec<DirFile>, AnyError> {
        let path = format!("{}/{}", dir, rel);
        let (q_path, q_dir, q_rel) = (sh::quote(&path), sh::quote(dir), sh::quote(rel));
        // one line per entry other than a directory: its kind (f a file, l a symlink, o anything
        // else), its executable bit, its size and its path below the dir
        let list = format!(
            r#"if [ -L {p} ]; then echo "is or sits behind a symlink, which is refused"
elif [ ! -d {p} ]; then echo "is not a directory"
elif [ "$(realpath {p} 2>/dev/null)" != "$(realpath {d} 2>/dev/null)/"{r} ]; then echo "is or sits behind a symlink, which is refused"
else echo ok; cd {p} && find . ! -type d | sort | while IFS= read -r f; do
  if [ -L "$f" ]; then echo "l 0 0 ${{f#./}}"
  elif [ ! -f "$f" ]; then echo "o 0 0 ${{f#./}}"
  else if [ -x "$f" ]; then x=1; else x=0; fi; echo "f $x $(($(wc -c < "$f"))) ${{f#./}}"; fi
done; fi"#,
            p = q_path,
            d = q_dir,
            r = q_rel,
        );
        let output = self
            .api
            .exec
            .output(
                "rooz-cfg-dir",
                self.container_id,
                None,
                Some(vec!["sh", "-c", &list]),
            )
            .await?;
        let mut lines = output.lines();
        match lines.next().map(str::trim) {
            Some("ok") => (),
            Some(why) => return Err(refused(rel, why)),
            None => return Err(refused(rel, "cannot be read")),
        }
        let mut listing = vec![];
        for line in lines {
            let mut parts = line.trim().splitn(4, ' ');
            let (kind, x, size, p) = match (
                parts.next(),
                parts.next(),
                parts.next().map(str::parse::<u64>),
                parts.next(),
            ) {
                (Some(kind), Some(x), Some(Ok(size)), Some(p)) => (kind, x, size, p),
                _ => return Err(refused(rel, "cannot be listed")),
            };
            if !filter.keeps(p) {
                continue;
            }
            match kind {
                "f" => listing.push((p.to_string(), size, x == "1")),
                "l" => {
                    return Err(refused(
                        &format!("{}/{}", rel, p),
                        "is a symlink, which is refused",
                    ));
                }
                _ => {
                    return Err(refused(&format!("{}/{}", rel, p), "is not a regular file"));
                }
            }
        }
        check_dir_listing(
            rel,
            &listing
                .iter()
                .map(|(p, size, _)| (p.clone(), *size))
                .collect::<Vec<_>>(),
        )?;
        let mut files = vec![];
        for (p, _, executable) in listing {
            files.push(DirFile {
                content: self.read_data_file(dir, &format!("{}/{}", rel, p)).await?,
                path: p,
                executable,
            });
        }
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config::{DirSource, MountSource};
    use std::{
        fs,
        os::unix::fs::symlink,
        path::{Path, PathBuf},
    };

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rooz-datafile-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("files")).unwrap();
        dir
    }

    async fn read(dir: &Path, rel: &str) -> Result<String, String> {
        LocalReader::default()
            .read_data_file(&dir.to_string_lossy(), rel)
            .await
            .map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn reads_a_regular_file_and_records_it_for_trust_checks() {
        let dir = scratch("ok");
        fs::write(dir.join("files/run.sh"), "echo hi\n").unwrap();
        let reader = LocalReader::default();
        let got = reader
            .read_data_file(&dir.to_string_lossy(), "files/run.sh")
            .await
            .unwrap();
        assert_eq!(got, "echo hi\n");
        assert_eq!(reader.read.lock().unwrap().len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn refuses_symlinks_to_files_and_through_directories() {
        let dir = scratch("link");
        let outside = scratch("outside");
        fs::write(outside.join("secret"), "key").unwrap();
        symlink(outside.join("secret"), dir.join("files/leak")).unwrap();
        symlink(&outside, dir.join("linkdir")).unwrap();
        for rel in ["files/leak", "linkdir/secret"] {
            let err = read(&dir, rel).await.unwrap_err();
            assert!(err.contains("symlink"), "{}: {}", rel, err);
        }
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[tokio::test]
    async fn refuses_oversized_binary_directories_and_missing() {
        let dir = scratch("bad");
        fs::write(dir.join("big"), vec![b'a'; MAX_DATA_FILE_BYTES + 1]).unwrap();
        fs::write(dir.join("exact"), vec![b'a'; MAX_DATA_FILE_BYTES]).unwrap();
        fs::write(dir.join("bin"), [0xff, 0xfe, 0x00]).unwrap();
        assert!(read(&dir, "big").await.unwrap_err().contains("larger than"));
        assert!(read(&dir, "exact").await.is_ok());
        assert!(read(&dir, "bin").await.unwrap_err().contains("UTF-8"));
        assert!(
            read(&dir, "files")
                .await
                .unwrap_err()
                .contains("regular file")
        );
        assert!(read(&dir, "nope").await.is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn resolves_relative_to_the_declaring_config() {
        let dir = scratch("resolve");
        fs::write(dir.join("files/a.conf"), "A").unwrap();
        fs::write(dir.join("files/s.conf"), "S").unwrap();
        let mut cfg: RoozCfg = serde_yaml::from_str(
            "data:\n  a:\n    file: files/a.conf\nsidecars:\n  s:\n    mounts:\n      /s:\n        file: files/s.conf\n",
        )
        .unwrap();
        let cfg_path = dir.join("rooz.yaml").to_string_lossy().into_owned();
        resolve_data_files(&LocalReader::default(), &cfg_path, &mut cfg)
            .await
            .unwrap();
        let contents: Vec<_> = cfg
            .data_values_mut()
            .into_iter()
            .filter_map(|v| match v {
                DataValue::FileContent { content, .. } => content.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(contents, vec!["A".to_string(), "S".to_string()]);

        let mut escaping: RoozCfg = serde_yaml::from_str("data:\n  a:\n    file: ../x\n").unwrap();
        assert!(
            resolve_data_files(&LocalReader::default(), &cfg_path, &mut escaping)
                .await
                .is_err()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    fn mounted(cfg: &RoozCfg) -> Vec<(String, String, bool)> {
        cfg.mounts
            .iter()
            .flatten()
            .filter_map(|(target, m)| match m {
                MountSource::InlineDataValue(DataValue::FileContent {
                    content,
                    executable,
                    ..
                }) => Some((
                    target.clone(),
                    content.clone().unwrap_or_default(),
                    executable.unwrap_or_default(),
                )),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_dir_becomes_a_file_mount_per_file_and_an_explicit_mount_wins() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("dir");
        fs::create_dir_all(dir.join("home/.config/nvim")).unwrap();
        fs::write(dir.join("home/.bashrc"), "B").unwrap();
        fs::write(dir.join("home/.config/nvim/init.lua"), "I").unwrap();
        fs::write(dir.join("home/run.sh"), "R").unwrap();
        fs::set_permissions(dir.join("home/run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.join("files/init.lua"), "mine").unwrap();
        let mut cfg: RoozCfg = serde_yaml::from_str(
            "mounts:\n  ~/:\n    dir: home\n  ~/.config/nvim/init.lua:\n    file: files/init.lua\n",
        )
        .unwrap();
        let cfg_path = dir.join("rooz.yaml").to_string_lossy().into_owned();
        let reader = LocalReader::default();
        resolve_data_files(&reader, &cfg_path, &mut cfg)
            .await
            .unwrap();
        assert_eq!(reader.read.lock().unwrap().len(), 4);
        cfg.expand_dir_mounts().unwrap();
        let mut got = mounted(&cfg);
        got.sort();
        assert_eq!(
            got,
            vec![
                ("~/.bashrc".into(), "B".into(), false),
                ("~/.config/nvim/init.lua".into(), "mine".into(), false),
                ("~/run.sh".into(), "R".into(), true),
            ]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_dir_holding_a_symlink_or_too_many_files_is_refused() {
        let dir = scratch("dirbad");
        fs::create_dir_all(dir.join("linked")).unwrap();
        symlink("/etc/passwd", dir.join("linked/passwd")).unwrap();
        fs::create_dir_all(dir.join("many")).unwrap();
        for i in 0..=MAX_DIR_FILES {
            fs::write(dir.join(format!("many/f{}", i)), "x").unwrap();
        }
        fs::write(dir.join("files/x"), "x").unwrap();
        let reader = LocalReader::default();
        let root = dir.to_string_lossy();
        for (rel, why) in [
            ("linked", "symlink"),
            ("many", "more than"),
            ("files/x", "not a directory"),
        ] {
            let err = reader
                .read_data_dir(&root, rel, &DirFilter::default())
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains(why), "{}: {}", rel, err);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn what_a_dir_filter_leaves_out_is_not_looked_at() {
        let dir = scratch("dirfilter");
        let home = dir.join("home");
        fs::create_dir_all(home.join(".git/objects")).unwrap();
        for i in 0..=MAX_DIR_FILES {
            fs::write(home.join(format!(".git/objects/o{}", i)), "x").unwrap();
        }
        symlink("/etc/passwd", home.join("passwd")).unwrap();
        fs::write(home.join("big.bin"), vec![b'x'; MAX_DATA_FILE_BYTES + 1]).unwrap();
        fs::write(home.join(".bashrc"), "rc").unwrap();
        fs::write(home.join("notes.bak"), "old").unwrap();
        let filter = DirFilter::new(&DirSource {
            dir: "home".into(),
            files: None,
            executable: None,
            once: None,
            include: None,
            exclude: Some(vec![
                ".git".into(),
                "passwd".into(),
                "*.bin".into(),
                "*.bak".into(),
            ]),
        })
        .unwrap();
        let files = LocalReader::default()
            .read_data_dir(&dir.to_string_lossy(), "home", &filter)
            .await
            .unwrap();
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            [".bashrc"]
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
