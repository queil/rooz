use colored::Colorize;

use crate::{api::WorkspaceApi, model::types::AnyError, util::labels::Labels};

impl<'a> WorkspaceApi<'a> {
    pub async fn stop(&self, workspace_key: &str) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), None);
        for c in self.api.container.get_running(&labels).await? {
            print!("Stopping container: {} ... ", c.names.unwrap().join(", "));
            self.api.container.stop(&c.id.unwrap()).await?;
            println!("{}", format!("OK").green())
        }
        Ok(())
    }

    pub async fn stop_all(&self) -> Result<(), AnyError> {
        self.api.container.stop_all().await
    }
}
