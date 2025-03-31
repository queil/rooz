use crate::{
    api::WorkspaceApi,
    model::types::AnyError,
    util::labels::{Labels, ROLE_WORK},
};
use colored::Colorize;

impl<'a> WorkspaceApi<'a> {
    pub async fn restart(
        &self,
        workspace_key: &str,
        all_containers: Option<bool>,
    ) -> Result<(), AnyError> {
        if let Some(true) = all_containers {
            self.stop(workspace_key).await?;
            self.start(workspace_key).await?;
        } else {
            let labels = Labels::new(Some(workspace_key), Some(ROLE_WORK));
            if let Some(c) = self.api.container.get_single(&labels).await? {
                let cid = &c.id.unwrap();
                let cname = &c.names.unwrap().join(", ");
                print!("Stopping container: {} ... ", cname);
                self.api.container.stop(cid).await?;
                println!("{}", format!("OK").green());
                print!("Starting container: {} ... ", cname);
                self.api.container.start(cid).await?;
                println!("{}", format!("OK").green())
            } else {
                eprintln!("Workspace not found {}", workspace_key);
            }
        };

        Ok(())
    }
}
