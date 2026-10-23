use crate::{
    config::config::ResolvedStep,
    util::labels::{self, Labels},
};
use bollard::Docker;
use bollard_stubs::models::{ContainerConfig, ImageSummary};
use bollard_stubs::query_parameters::{
    CommitContainerOptions, ListImagesOptions, RemoveImageOptions,
};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fmt::Debug};

// A container's install progress: an image committed after each step that ran, named by everything
// that shaped the container up to and including that step. A later attempt starts from the
// longest one that still matches and runs the steps after it, so a failed step does not throw away
// what the steps before it downloaded.
#[derive(Clone)]
pub struct Checkpoints {
    workspace_key: String,
    container: String,
    inputs: String,
}

fn hash(value: &impl Debug) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{:?}", value));
    format!("{:x}", hasher.finalize())
}

impl Checkpoints {
    // `inputs` is whatever outside the steps changes what they leave behind: the base image,
    // the user, the environment the steps run with.
    pub fn new(workspace_key: &str, container: &str, inputs: &impl Debug) -> Self {
        Self {
            workspace_key: workspace_key.to_string(),
            container: container.to_string(),
            inputs: hash(inputs),
        }
    }

    fn labels(&self) -> Labels {
        Labels::from(&[
            Labels::workspace(&self.workspace_key),
            Labels::role(labels::INSTALL_CHECKPOINT_ROLE),
            Labels::container(&self.container),
        ])
    }

    // the first `done` steps and what they ran on
    fn key(&self, steps: &[ResolvedStep], done: usize) -> String {
        hash(&(&self.inputs, &steps[..done]))
    }

    // How many steps are already done, and the image that has them. The image is only ever one
    // rooz committed and recorded itself: a tag or a label is no evidence of what carries it,
    // since the image namespace is shared and anybody with engine access can write either.
    pub async fn find(&self, client: &Docker, steps: &[ResolvedStep]) -> Option<(usize, String)> {
        let record = self.read_record();
        for done in (1..=steps.len()).rev() {
            let key = self.key(steps, done);
            let Some(id) = record.get(&key) else {
                continue;
            };
            let mut expected = self.labels();
            expected.append(Labels::build_key(&key));
            let carried = match client.inspect_image(id).await {
                Ok(inspect) if same_id(inspect.id.as_deref(), id) => {
                    inspect.config.and_then(|c| c.labels).unwrap_or_default()
                }
                _ => continue,
            };
            if expected.is_subset_of(&carried) {
                return Some((done, id.clone()));
            }
        }
        None
    }

    // Best effort: losing a checkpoint costs time, not correctness, so it never fails an install.
    pub async fn save(
        &self,
        client: &Docker,
        container_id: &str,
        steps: &[ResolvedStep],
        done: usize,
    ) {
        let key = self.key(steps, done);
        let mut labels = self.labels();
        labels.append(Labels::build_key(&key));
        let committed = client
            .commit_container(
                CommitContainerOptions {
                    container: Some(container_id.to_string()),
                    repo: Some(format!(
                        "localhost/rooz/{}/{}-checkpoint",
                        self.workspace_key, self.container
                    )),
                    tag: Some(key[..12].to_string()),
                    pause: false,
                    ..Default::default()
                },
                ContainerConfig {
                    labels: Some(labels.into()),
                    ..Default::default()
                },
            )
            .await;
        match committed {
            Ok(image) => {
                // the id is the only thing a later attempt can trust, and only rooz knows which
                // ids it committed: keep them where the engine cannot be written to
                let mut record = self.read_record();
                record.insert(key, image.id);
                self.write_record(&record);
            }
            Err(e) => log::warn!(
                "Could not save install checkpoint of {} after step {}: {}",
                self.container,
                done,
                e
            ),
        }
    }

    pub async fn clear(&self, client: &Docker) {
        match list(client, &self.labels()).await {
            Ok(images) => remove(client, &images).await,
            Err(e) => log::debug!("Could not list install checkpoints: {}", e),
        }
        if let Some(path) = self.record_path()
            && let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::debug!("Could not forget install checkpoints: {}", e);
        }
    }

    // The ids of the checkpoint images rooz committed, kept off the engine: an image label, a
    // container label and a volume label are all writable by anybody with engine access, so none
    // of them is evidence that rooz built an image. Same reasoning as the age identity, which
    // lives in the operator's own directory rather than in a volume.
    fn record_path(&self) -> Option<std::path::PathBuf> {
        let dir = crate::util::identity::key_path()
            .ok()?
            .parent()?
            .join("checkpoints");
        let name = hash(&(&self.workspace_key, &self.container));
        Some(dir.join(format!("{}.record", name)))
    }

    fn read_record(&self) -> HashMap<String, String> {
        let body = match self.record_path() {
            Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
            None => String::new(),
        };
        body.lines()
            .filter_map(|line| {
                let (key, id) = line.split_once(' ')?;
                Some((key.to_string(), id.to_string()))
            })
            .collect()
    }

    fn write_record(&self, record: &HashMap<String, String>) {
        let Some(path) = self.record_path() else {
            return;
        };
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            log::debug!("Could not keep install checkpoints: {}", e);
            return;
        }
        let body = record
            .iter()
            .map(|(key, id)| format!("{} {}\n", key, id))
            .collect::<String>();
        if let Err(e) = std::fs::write(&path, body) {
            log::debug!("Could not keep install checkpoints: {}", e);
        }
    }
}

// Engines are inconsistent about the 'sha256:' prefix, and a name is not an id.
fn same_id(found: Option<&str>, expected: &str) -> bool {
    fn bare(value: &str) -> &str {
        value.strip_prefix("sha256:").unwrap_or(value)
    }
    match found {
        Some(found) => !bare(found).is_empty() && bare(found) == bare(expected),
        None => false,
    }
}

pub async fn list(
    client: &Docker,
    labels: &Labels,
) -> Result<Vec<ImageSummary>, bollard::errors::Error> {
    client
        .list_images(Some(ListImagesOptions {
            filters: Some(labels.clone().into()),
            all: true,
            ..Default::default()
        }))
        .await
}

pub fn is_checkpoint(image: &ImageSummary) -> bool {
    image.labels.get(labels::ROLE).map(String::as_str) == Some(labels::INSTALL_CHECKPOINT_ROLE)
}

// By tag, which untags and leaves the layers to whatever still uses them - the container, or an
// image built from the checkpoint. Failures are ignored: what is in use stays until its user goes.
pub async fn remove(client: &Docker, images: &[ImageSummary]) {
    for image in images {
        let names = image
            .repo_tags
            .iter()
            .filter(|t| !t.starts_with("<none>"))
            .cloned()
            .collect::<Vec<_>>();
        let names = if names.is_empty() {
            vec![image.id.clone()]
        } else {
            names
        };
        for name in names {
            if let Err(e) = client
                .remove_image(&name, None::<RemoveImageOptions>, None)
                .await
            {
                log::debug!("Kept install checkpoint {}: {}", name, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_compare_across_the_sha256_prefix_and_never_match_a_name() {
        assert!(same_id(Some("sha256:abc"), "abc"));
        assert!(same_id(Some("abc"), "sha256:abc"));
        assert!(!same_id(Some("sha256:abc"), "def"));
        assert!(!same_id(Some(""), ""));
        assert!(!same_id(None, "abc"));
    }

    fn step(name: &str, run: &str) -> ResolvedStep {
        ResolvedStep {
            name: name.into(),
            run: run.into(),
            skip_if: None,
            root: false,
        }
    }

    fn checkpoints(base: &str) -> Checkpoints {
        Checkpoints::new("ws", "c", &(base, "1000"))
    }

    #[test]
    fn a_prefix_key_ignores_the_steps_after_it() {
        let a = [step("a", "1"), step("b", "2")];
        let b = [step("a", "1"), step("b", "changed"), step("c", "3")];
        let c = checkpoints("img");
        assert_eq!(c.key(&a, 1), c.key(&b, 1));
        assert_ne!(c.key(&a, 2), c.key(&b, 2));
    }

    #[test]
    fn the_inputs_outside_the_steps_are_part_of_the_key() {
        let steps = [step("a", "1")];
        assert_ne!(
            checkpoints("img1").key(&steps, 1),
            checkpoints("img2").key(&steps, 1)
        );
    }

    #[test]
    fn a_step_flag_changes_the_key() {
        let plain = [step("a", "1")];
        let root = [ResolvedStep {
            root: true,
            ..step("a", "1")
        }];
        let c = checkpoints("img");
        assert_ne!(c.key(&plain, 1), c.key(&root, 1));
    }
}
