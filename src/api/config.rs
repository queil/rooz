use std::io;

use crate::{
    config::config::{
        ConfigType, DataValue, FileFormat, MAX_DATA_FILE_BYTES, RoozCfg, SystemConfig,
        validate_relative_config_path, validate_relative_file_path,
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
}

fn refused(rel: &str, why: &str) -> AnyError {
    format!("'{}' {}", rel, why).into()
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

    fn edit_error(&self, message: &str) {
        eprintln!("{}", "Error: Invalid configuration".bold().red());
        eprintln!("{}", message.red());
        eprintln!("Press any key to continue editing...");
        io::stdin().read_line(&mut String::new()).unwrap();
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
                    self.edit_error(&err.to_string());
                    continue;
                }
            };
            edited_config = match RoozCfg::from_string(&edited_body, format) {
                Ok(c) => c,
                Err(err) => {
                    self.edit_error(&err.to_string());
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
                ));
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
                    self.edit_error(&err.to_string());
                    continue;
                }
            };
            edited_config = match SystemConfig::from_string(&edited_body) {
                Ok(c) => c,
                Err(err) => {
                    self.edit_error(&err.to_string());
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
            let mut base = RoozCfg::deserialize_config(&base_body, base_fmt)?
                .ok_or_else(|| format!("Failed to parse base '{}': invalid config", base_path))?;
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

        if let Some(mut cfg) = RoozCfg::deserialize_config(&body, file_format)? {
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
            return Ok(Some(ConfigBody {
                body,
                bases,
                merged: Some(merged),
            }));
        }

        Ok(Some(ConfigBody {
            body,
            bases: None,
            merged: None,
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
