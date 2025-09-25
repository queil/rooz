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

use super::ConfigApi;

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
                Some(vec![RoozVolume::workspace_config_read(
                    workspace_key,
                    "/etc/rooz",
                )
                .to_mount(None)]),
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
}
