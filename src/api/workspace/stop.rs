use crate::{api::WorkspaceApi, model::types::AnyError, util::labels::Labels};

impl<'a> WorkspaceApi<'a> {
    pub async fn stop(&self, workspace_key: &str) -> Result<(), AnyError> {
        let labels = Labels::new(Some(workspace_key), None);
        for c in self.api.container.get_all(&labels).await? {
            self.api.container.stop(&c.id.unwrap()).await?;
        }
        Ok(())
    }

    pub async fn stop_all(&self) -> Result<(), AnyError> {
        let labels = Labels::default();
        for c in self.api.container.get_all(&labels).await? {
            self.api.container.stop(&c.id.unwrap()).await?;
        }
        Ok(())
    }
}
