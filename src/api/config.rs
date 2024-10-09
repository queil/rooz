use std::io;

use crate::{
    config::config::{FileFormat, RoozCfg},
    model::types::AnyError,
};

use age::x25519::Identity;
use colored::Colorize;

use super::ConfigApi;

impl<'a> ConfigApi<'a> {

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
        self.encrypt(& mut edited_config, identity).await?;
        Ok((edited_config, edited_body))
    }
}
