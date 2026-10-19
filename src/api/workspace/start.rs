use crate::{api::WorkspaceApi, model::types::AnyError, util::labels::Labels};
use colored::Colorize;

impl<'a> WorkspaceApi<'a> {
    pub async fn start(&self, workspace_key: &str) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);

        for c in self.api.container.get_all(&labels).await? {
            let container_name = c.names.clone().unwrap_or_default().join(", ");

            print!("Starting container: {} ... ", container_name);
            let container_id = c.id.unwrap();
            self.api.container.start(&container_id).await?;
            println!("{}", "OK".green())
        }
        Ok(())
    }
}
