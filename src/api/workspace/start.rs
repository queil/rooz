use crate::config::runtime::RuntimeConfig;
use crate::util::labels;
use crate::{api::WorkspaceApi, constants, model::types::AnyError, util::labels::Labels};
use colored::Colorize;

impl<'a> WorkspaceApi<'a> {
    pub async fn start(
        &self,
        workspace_key: &str,
        cfg: Option<(&RuntimeConfig, &str)>,
    ) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);

        //TODO here need to pass real_mounts but per container, for that runtimeconfig needs to be extended with sidecars real mounts data

        for c in self.api.container.get_all(&labels).await? {
            let names = c.names.clone().unwrap();
            let container_name = match names.as_slice() {
                [name] => name,
                name => panic!("Unexpected container name(s): {:?}", name),
            };

            print!("Starting container: {} ... ", container_name);
            let container_id = c.id.unwrap();
            self.api.container.start(&container_id).await?;
            if let Some((config, uid)) = cfg {
                let rooz_container_name = &c.labels.unwrap()[labels::CONTAINER];
                let (real_mounts, uid) = if rooz_container_name == constants::DEFAULT_CONTAINER_NAME
                {
                    (&config.real_mounts, uid)
                } else {
                    let sidecar = &config.sidecars[rooz_container_name];
                    (&sidecar.real_mounts, sidecar.user.as_str())
                };

                self.api
                    .exec
                    .symlink_files(&container_id, real_mounts, uid)
                    .await?;
            }
            println!("{}", format!("OK").green())
        }
        Ok(())
    }
}
