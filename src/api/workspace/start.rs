use crate::{api::WorkspaceApi, model::types::AnyError, util::labels::Labels};
use colored::Colorize;

impl<'a> WorkspaceApi<'a> {
    pub async fn start(&self, workspace_key: &str) -> Result<(), AnyError> {
        let labels = Labels::from(&[Labels::workspace(workspace_key)]);

        for c in self.api.container.get_all(&labels).await? {
            print!("Starting container: {} ... ", c.names.unwrap().join(", "));
            self.api.container.start(&c.id.unwrap()).await?;
            println!("{}", format!("OK").green())
        }
        Ok(())
    }
}
