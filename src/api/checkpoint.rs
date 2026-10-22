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

    // How many steps are already done, and the image that has them. Steps changed since, or an
    // other base image, match nothing.
    pub async fn find(&self, client: &Docker, steps: &[ResolvedStep]) -> Option<(usize, String)> {
        let images = list(client, &self.labels())
            .await
            .map_err(|e| log::debug!("Could not list install checkpoints: {}", e))
            .ok()?;
        let by_key = images
            .into_iter()
            .filter_map(|i| Some((i.labels.get(labels::BUILD_KEY)?.clone(), i.id)))
            .collect::<HashMap<_, _>>();
        (1..=steps.len())
            .rev()
            .find_map(|done| Some((done, by_key.get(&self.key(steps, done))?.clone())))
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
        if let Err(e) = committed {
            log::warn!(
                "Could not save install checkpoint of {} after step {}: {}",
                self.container,
                done,
                e
            );
        }
    }

    pub async fn clear(&self, client: &Docker) {
        match list(client, &self.labels()).await {
            Ok(images) => remove(client, &images).await,
            Err(e) => log::debug!("Could not list install checkpoints: {}", e),
        }
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
