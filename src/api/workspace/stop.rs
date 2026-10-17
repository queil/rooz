use colored::Colorize;

use crate::{api::WorkspaceApi, model::types::AnyError, util::labels::Labels};

impl<'a> WorkspaceApi<'a> {
    pub async fn stop(&self, workspace_key: &str) -> Result<(), AnyError> {
        self.stop_matching(&Labels::from(&[Labels::workspace(workspace_key)]))
            .await
    }

    pub async fn stop_all(&self) -> Result<(), AnyError> {
        self.stop_matching(&Labels::default()).await
    }

    async fn stop_matching(&self, labels: &Labels) -> Result<(), AnyError> {
        for c in self.api.container.get_running(labels).await? {
            print!("Stopping container: {} ... ", c.names.unwrap().join(", "));
            self.api.container.stop(&c.id.unwrap()).await?;
            println!("{}", "OK".to_string().green())
        }
        Ok(())
    }
}
