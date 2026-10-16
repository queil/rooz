use std::io::Write;

use bollard::{container::LogOutput, query_parameters::LogsOptions};
use futures::StreamExt;

use crate::{api::Api, constants, model::types::AnyError, util::labels::Labels};

impl<'a> Api<'a> {
    pub async fn logs(
        &self,
        workspace_key: &str,
        container_name: Option<&str>,
        follow: bool,
        tail: Option<&str>,
    ) -> Result<(), AnyError> {
        let container_name = container_name.unwrap_or(constants::DEFAULT_CONTAINER_NAME);
        let container = self
            .container
            .get_single(&Labels::from(&[
                Labels::workspace(workspace_key),
                Labels::container(container_name),
            ]))
            .await?
            .ok_or(format!(
                "Container '{}' of workspace '{}' not found. Check 'rooz status {}'.",
                container_name, workspace_key, workspace_key
            ))?;
        let id = container.id.as_deref().unwrap();

        let mut stream = self.client.logs(
            id,
            Some(LogsOptions {
                follow,
                stdout: true,
                stderr: true,
                tail: tail.unwrap_or("all").to_string(),
                ..Default::default()
            }),
        );

        while let Some(item) = stream.next().await {
            match item? {
                LogOutput::StdErr { message } => {
                    let mut e = std::io::stderr();
                    e.write_all(&message)?;
                    e.flush()?;
                }
                other => {
                    let mut o = std::io::stdout();
                    o.write_all(&other.into_bytes())?;
                    o.flush()?;
                }
            }
        }
        Ok(())
    }
}
