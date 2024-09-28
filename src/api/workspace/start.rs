use crate::{api::WorkspaceApi, labels::Labels, model::types::AnyError};

impl<'a> WorkspaceApi<'a> {
    pub async fn start_workspace(&self, workspace_key: &str) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), None);
        for c in self.api.container.get_all(&labels).await? {
            self.api.container.start(&c.id.unwrap()).await?;
        }
        Ok(())
    }
}
