use crate::{api::WorkspaceApi, model::types::AnyError, util::labels::Labels};
use colored::Colorize;

impl<'a> WorkspaceApi<'a> {
    pub async fn start(&self, workspace_key: &str) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);

        for c in self.api.container.get_all(&labels).await? {
            let names = c.names.clone().unwrap();
            let container_name = match names.as_slice() {
                [name] => name,
                name => panic!("Unexpected container name(s): {:?}", name),
            };

            print!("Starting container: {} ... ", container_name);
            let container_id = c.id.unwrap();
            self.api.container.start(&container_id).await?;
            println!("{}", "OK".green())
        }
        Ok(())
    }
}
