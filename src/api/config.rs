use std::io;

use crate::{
    config::config::{ConfigType, FileFormat, RoozCfg, SystemConfig},
    constants,
    model::{
        types::AnyError,
        volume::{RoozVolume, RoozVolumeRole},
    },
    util::labels::Labels,
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
}

impl<'a> ConfigApi<'a> {
    pub async fn store(
        &self,
        workspace_key: &str,
        origin: &str,
        body: &str,
    ) -> Result<(), AnyError> {
        let config_vol = RoozVolume::config_data(
            workspace_key,
            "/etc/rooz",
            Some(
                [(ConfigType::Body.file_path().to_string(), body.to_string())]
                    .into_iter()
                    .collect(),
            ),
            Some(Labels::from(&[Labels::config_origin(origin)])),
            Some(RoozVolumeRole::WorkspaceConfig),
        );
        self.api
            .volume
            .ensure_mounts(&vec![config_vol], None, Some(constants::ROOT_UID))
            .await?;
        Ok(())
    }

    pub async fn store_bases(&self, workspace_key: &str, body: &str) -> Result<(), AnyError> {
        let config_vol = RoozVolume::config_data(
            workspace_key,
            "/etc/rooz",
            Some(
                [(ConfigType::Bases.file_path().to_string(), body.to_string())]
                    .into_iter()
                    .collect(),
            ),
            None,
            Some(RoozVolumeRole::WorkspaceConfig),
        );
        self.api
            .volume
            .ensure_mounts(&vec![config_vol], None, Some(constants::ROOT_UID))
            .await?;
        Ok(())
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
                    "ls /etc/rooz/{} > /dev/null 2>&1 && cat /etc/rooz/{} || echo ''",
                    config_path, config_path
                )
                .into(),
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
        let config_vol = RoozVolume::config_data(
            workspace_key,
            "/etc/rooz",
            Some(
                [(
                    ConfigType::Runtime.file_path().to_string(),
                    data.to_string(),
                )]
                .into_iter()
                .collect(),
            ),
            None,
            Some(RoozVolumeRole::WorkspaceConfig),
        );
        self.api
            .volume
            .ensure_mounts(&vec![config_vol], None, Some(constants::ROOT_UID))
            .await?;
        Ok(())
    }

    fn edit_error(&self, message: &str) -> () {
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

            match (&edited_config.vars, &edited_config.secrets) {
                (Some(vars), Some(secrets)) => {
                    if let Some(duplicate_key) =
                        vars.keys().find(|k| secrets.contains_key(&k.to_string()))
                    {
                        self.edit_error(&format!(
                            "The key: '{}' can be only defined in either vars or secrets.",
                            &duplicate_key.to_string()
                        ));
                        continue;
                    }
                }
                _ => (),
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

    const MAX_EXTENDS_DEPTH: usize = 2;

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

        if depth >= Self::MAX_EXTENDS_DEPTH {
            return Err(
                format!("bases nesting too deep (limit {})", Self::MAX_EXTENDS_DEPTH).into(),
            );
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

            let base_fmt = FileFormat::from_path(base_path);
            let base = RoozCfg::deserialize_config(&base_body, base_fmt)?
                .ok_or_else(|| format!("Failed to parse base '{}': invalid config", base_path))?;

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
        exact_path: Option<&str>,
    ) -> Result<Option<ConfigBody>, AnyError> {
        let file_path = match exact_path {
            Some(p) => format!("{}/{}", clone_dir, p.to_string()),
            None => format!("{}/.rooz.{}", clone_dir, file_format.to_string()),
        };

        let ls_cmd = format!(
            "ls {} > /dev/null 2>&1 && cat `ls {} | head -1`",
            file_path, file_path
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
            return match exact_path {
                Some(p) => Err(format!("Config file '{}' not found or empty", p).into()),
                None => Ok(None),
            };
        }

        if let (Some(_), Some(cfg)) = (exact_path, RoozCfg::deserialize_config(&body, file_format)?)
        {
            if cfg.bases.is_some() {
                let reader = ContainerReader {
                    api: self.api,
                    container_id,
                };
                let (merged, individual_bases) = self
                    .resolve_extends_chain(&reader, &file_path, cfg, 0)
                    .await?;
                let bases_yaml = individual_bases
                    .iter()
                    .map(|(path, b)| {
                        b.to_string(file_format)
                            .map(|yaml| format!("# {}\n{}", path, yaml))
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .join("\n---\n");
                let bases_storage = if bases_yaml.is_empty() {
                    None
                } else {
                    Some(bases_yaml)
                };
                return Ok(Some(ConfigBody {
                    body,
                    bases: bases_storage,
                    merged: Some(merged),
                }));
            }
        }

        Ok(Some(ConfigBody {
            body,
            bases: None,
            merged: None,
        }))
    }

    pub async fn try_read_config(
        &self,
        container_id: &str,
        clone_dir: &str,
    ) -> Result<Option<(String, Option<String>, FileFormat)>, AnyError> {
        let rooz_cfg = if let Some(cb) = self
            .read_config_body(&container_id, &clone_dir, FileFormat::Yaml, None)
            .await?
        {
            log::debug!("Config file found (YAML)");
            Some((cb.body, None, FileFormat::Yaml))
        } else {
            log::debug!("No valid config file found");
            None
        };
        Ok(rooz_cfg)
    }
}

pub struct LocalReader;

#[async_trait::async_trait]
impl ConfigReader for LocalReader {
    async fn read_file(&self, path: &str) -> Result<String, AnyError> {
        std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read extends '{}': {}", path, e).into())
    }
}

pub struct ContainerReader<'a> {
    pub api: &'a Api<'a>,
    pub container_id: &'a str,
}

#[async_trait::async_trait]
impl<'a> ConfigReader for ContainerReader<'a> {
    async fn read_file(&self, path: &str) -> Result<String, AnyError> {
        let cat_cmd = format!("cat '{}'", path);
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
}
