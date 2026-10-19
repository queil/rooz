use std::collections::HashMap;

use bollard::query_parameters::ListVolumesOptions;

use crate::{
    api::WorkspaceApi,
    cli::WorkParams,
    cmd::plan::PlanMode,
    config::{config::ConfigType, runtime::RuntimeConfig},
    model::types::{AnyError, VolumeSpec},
    util::{
        id,
        labels::{Labels, ROLE, WORKSPACE_CONFIG_ROLE, WORKSPACE_KEY},
    },
};

// Volume names embed the workspace key as the first segment ('rooz_<key>_work',
// 'rooz_<key>_<path>_data'); segments are joined with '_', which a sanitized segment never
// contains. Anything else is not ours to rename.
pub fn renamed_volume(name: &str, from: &str, to: &str) -> Option<String> {
    name.strip_prefix(&format!("rooz_{from}_"))
        .map(|rest| format!("rooz_{to}_{rest}"))
}

pub(crate) struct VolumeMove {
    pub from: String,
    pub to: String,
    pub labels: HashMap<String, String>,
}

impl<'a> WorkspaceApi<'a> {
    pub(crate) async fn volumes_to_move(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<VolumeMove>, AnyError> {
        let volumes = self
            .api
            .client
            .list_volumes(Some(ListVolumesOptions {
                filters: Some(Labels::from(&[Labels::workspace(from)]).into()),
            }))
            .await?
            .volumes
            .unwrap_or_default();

        let mut moves = Vec::new();
        for v in volumes {
            // the config volume is rewritten under the new key by the rebuild
            if v.labels.get(ROLE).map(String::as_str) == Some(WORKSPACE_CONFIG_ROLE) {
                continue;
            }
            let new_name = renamed_volume(&v.name, from, to).ok_or(format!(
                "Volume '{}' belongs to '{}' but its name does not carry the workspace name, so it \
                 cannot be moved. Remove it or leave the workspace where it is.",
                v.name, from
            ))?;
            let mut labels = v.labels.clone();
            labels.insert(WORKSPACE_KEY.to_string(), to.to_string());
            moves.push(VolumeMove {
                from: v.name,
                to: new_name,
                labels,
            });
        }
        moves.sort_by(|a, b| a.from.cmp(&b.from));
        Ok(moves)
    }

    // What the rebuild is given besides the stored config file. A workspace made from flags alone
    // has no file, so what its flags set is carried over from the stored runtime config.
    pub(crate) async fn move_params(
        &self,
        from: &str,
        has_config_file: bool,
        allow_secrets: Option<bool>,
    ) -> Result<WorkParams, AnyError> {
        let git_ssh_url = self.stored_git_ssh_url(from).await?;
        let mut params = WorkParams {
            git_ssh_url,
            allow_secrets,
            ..Default::default()
        };
        if !has_config_file {
            let rt =
                RuntimeConfig::from_string(self.config.read(from, &ConfigType::Runtime).await?)?;
            params.image = Some(rt.image);
            params.user = Some(rt.user);
            params.uid = Some(rt.uid as u32);
            params.caches = Some(rt.caches).filter(|c| !c.is_empty());
            params.privileged = Some(rt.privileged);
        }
        Ok(params)
    }

    // move never replaces
    pub(crate) async fn ensure_name_free(&self, to: &str) -> Result<(), AnyError> {
        let dest = Labels::from(&[Labels::workspace(to)]);
        if !self.api.container.get_all(&dest).await?.is_empty()
            || !self
                .api
                .client
                .list_volumes(Some(ListVolumesOptions {
                    filters: Some(dest.clone().into()),
                }))
                .await?
                .volumes
                .unwrap_or_default()
                .is_empty()
        {
            return Err(format!(
                "Workspace '{}' already exists. 'move' never replaces one: remove it first with \
                 'rooz rm {}'.",
                to, to
            )
            .into());
        }
        Ok(())
    }

    pub async fn move_workspace(
        &self,
        from: &str,
        to: &str,
        dry_run: bool,
        allow_secrets: Option<bool>,
    ) -> Result<(), AnyError> {
        id::validate_workspace_name(to)?;
        if from == to {
            return Err("The new name is the same as the current one.".into());
        }

        self.api
            .volume
            .get_single(&Labels::workspace_config(from))
            .await?
            .ok_or(format!("Workspace not found: {}", from))?;

        self.ensure_name_free(to).await?;

        let moves = self.volumes_to_move(from, to).await?;
        for m in &moves {
            let spec = VolumeSpec {
                name: m.to.clone(),
                labels: None,
            };
            if self.api.volume.exists_for(&spec).await? {
                return Err(format!(
                    "Volume '{}' already exists and is not part of a workspace named '{}'. Remove \
                     it and try again.",
                    m.to, to
                )
                .into());
            }
        }

        let config_source = self.stored_config_source(from, false, false).await?;
        let params = self
            .move_params(from, config_source.is_some(), allow_secrets)
            .await?;

        // resolves and gates the configuration under the new name exactly as the rebuild will,
        // before anything is copied
        let plan = self
            .plan_new(to, &params, config_source.clone(), PlanMode::New)
            .await?;

        if dry_run {
            println!("Volumes to copy:");
            for m in &moves {
                println!("  {} -> {}", m.from, m.to);
            }
            println!("\n{}", plan);
            println!("Then '{}' is removed.", from);
            return Ok(());
        }

        self.stop(from).await?;

        let built: Result<(), AnyError> = async {
            for m in &moves {
                println!("Copying {} -> {}", m.from, m.to);
                self.api
                    .volume
                    .ensure_volume(&VolumeSpec {
                        name: m.to.clone(),
                        labels: Some(Labels::new(m.labels.clone())),
                    })
                    .await?;
                self.api.volume.copy_volume(&m.from, &m.to).await?;
            }
            self.new(to, &params, config_source, false, false).await?;
            Ok(())
        }
        .await;

        if let Err(e) = built {
            if let Err(cleanup) = self.remove(to, false, true).await {
                eprintln!(
                    "WARN: could not remove the partial workspace '{}': {}. Remove it with \
                     'rooz rm {} --force'.",
                    to, cleanup, to
                );
            }
            return Err(format!("Move failed; '{}' is untouched. {}", from, e).into());
        }

        self.remove(from, false, true).await?;
        println!("Moved '{}' to '{}'.", from, to);
        Ok(())
    }
}

impl<'a> WorkspaceApi<'a> {
    // Run on the receiving engine: whether a moved workspace can land here. The platform comes
    // back so the sending side can warn about an architecture change.
    pub async fn preflight_destination(&self, name: &str) -> Result<String, AnyError> {
        id::validate_workspace_name(name)?;
        self.ensure_name_free(name).await?;
        self.api.volume.require_ssh_key().await?;
        Ok(self.api.container.backend.platform.to_string())
    }

    // Run on the receiving engine: creates the volume (it must not exist) and fills it from a
    // tar stream on stdin, which is how the sending side's copy arrives.
    pub async fn receive_volume(
        &self,
        name: &str,
        labels: HashMap<String, String>,
    ) -> Result<(), AnyError> {
        use futures::stream;
        use tokio::io::AsyncReadExt;

        let created = self
            .api
            .volume
            .ensure_volume(&VolumeSpec {
                name: name.to_string(),
                labels: Some(Labels::new(labels)),
            })
            .await?;
        if !matches!(created, crate::model::types::VolumeResult::Created) {
            return Err(format!("Volume '{}' already exists on this engine.", name).into());
        }

        let archive_container = self
            .api
            .container
            .volume_archive_container(name, "/to", false)
            .await?;
        let input = stream::try_unfold(tokio::io::stdin(), |mut stdin| async move {
            let mut buf = vec![0u8; 256 * 1024];
            let n = stdin.read(&mut buf).await?;
            if n == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            buf.truncate(n);
            Ok(Some((bytes::Bytes::from(buf), stdin)))
        });
        let result = self
            .api
            .container
            .upload_tar_stream(&archive_container, "/to", input)
            .await;
        self.api
            .container
            .remove(&archive_container, true)
            .await
            .ok();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::renamed_volume;

    #[test]
    fn volumes_are_renamed_by_their_workspace_segment() {
        assert_eq!(
            renamed_volume("rooz_old_work", "old", "new").as_deref(),
            Some("rooz_new_work")
        );
        assert_eq!(
            renamed_volume("rooz_old_---dir_data", "old", "new").as_deref(),
            Some("rooz_new_---dir_data")
        );
    }

    #[test]
    fn another_workspace_with_a_longer_name_is_not_matched() {
        assert_eq!(
            renamed_volume("rooz_old-b_work", "old-b", "x").as_deref(),
            Some("rooz_x_work")
        );
        assert_eq!(renamed_volume("rooz_older_work", "old", "new"), None);
        assert_eq!(renamed_volume("something-else", "old", "new"), None);
    }
}
