use crate::util::labels::Labels;
use crate::{api::ImageApi, model::types::AnyError};
use bollard::errors::Error::DockerResponseServerError;
use bollard::models::CreateImageInfo;
use bollard::service::ImageInspect;
use bollard::{errors::Error, query_parameters::CreateImageOptions};
use bollard_stubs::models::ImageSummary;
use bollard_stubs::query_parameters::{ListImagesOptions, RemoveImageOptions};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::{QueueableCommand, cursor, terminal};
use futures::StreamExt;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::io::{Write, stdout};

#[derive(Debug)]
pub struct ImageInfo {
    pub id: String,
    pub platform: Option<String>,
}

const BAR_WIDTH: usize = 20;

// Bytes of one layer as its download reports them: all layers summed make the image's progress.
fn add_layer_bytes(
    bytes: &mut IndexMap<String, (i64, i64)>,
    id: &str,
    status: &str,
    current: Option<i64>,
    total: Option<i64>,
) -> (i64, i64) {
    let entry = bytes.entry(id.to_string()).or_insert((0, 0));
    if status.starts_with("Downloading") {
        if let Some(total) = total {
            entry.1 = entry.1.max(total);
        }
        if let Some(current) = current {
            entry.0 = current.min(entry.1.max(current));
        }
    } else if status.starts_with("Extracting")
        || status.starts_with("Verifying")
        || status.contains("complete")
        || status.contains("exists")
    {
        entry.0 = entry.1;
    }
    bytes
        .values()
        .fold((0, 0), |(c, t), (lc, lt)| (c + lc, t + lt))
}

struct LayerState {
    status: String,
    current: Option<i64>,
    total: Option<i64>,
}

pub struct PullProgress {
    image: String,
    layers: IndexMap<String, LayerState>,
    layer_bytes: IndexMap<String, (i64, i64)>,
    finished: bool,
    drawn_lines: u16,
    // under the live install view: one line, no bars drawing over its panes
    quiet: bool,
}

impl Drop for PullProgress {
    fn drop(&mut self) {
        if self.quiet {
            crate::util::live::pull_end(&self.image, self.finished);
        }
    }
}

impl PullProgress {
    pub fn new(image: &str) -> Self {
        if crate::util::live::is_live() {
            crate::util::live::pull_begin(image);
            return Self {
                image: image.to_string(),
                layer_bytes: IndexMap::new(),
                finished: false,
                layers: IndexMap::new(),
                drawn_lines: 0,
                quiet: true,
            };
        }
        let mut out = stdout();
        out.queue(SetForegroundColor(Color::White)).unwrap();
        out.queue(Print("⬇ Pulling ")).unwrap();
        out.queue(SetForegroundColor(Color::Cyan)).unwrap();
        out.queue(crossterm::style::SetAttribute(
            crossterm::style::Attribute::Bold,
        ))
        .unwrap();
        out.queue(Print(image)).unwrap();
        out.queue(ResetColor).unwrap();
        out.queue(Print("\n")).unwrap();
        out.flush().unwrap();
        Self {
            image: image.to_string(),
            layer_bytes: IndexMap::new(),
            finished: false,
            layers: IndexMap::new(),
            drawn_lines: 0,
            quiet: false,
        }
    }

    pub fn update(&mut self, id: &str, status: &str, current: Option<i64>, total: Option<i64>) {
        if self.quiet {
            let (current, total) =
                add_layer_bytes(&mut self.layer_bytes, id, status, current, total);
            crate::util::live::pull_progress(&self.image, current, total);
            return;
        }
        self.layers.insert(
            id.to_string(),
            LayerState {
                status: status.to_string(),
                current,
                total,
            },
        );
        self.render();
    }

    fn render(&mut self) {
        let mut out = stdout();

        if self.drawn_lines > 0 {
            out.queue(cursor::MoveUp(self.drawn_lines)).unwrap();
        }

        for (id, layer) in &self.layers {
            out.queue(terminal::Clear(terminal::ClearType::CurrentLine))
                .unwrap();

            let (bar, pct, size_str, color) = if let (Some(cur), Some(tot)) =
                (layer.current, layer.total)
            {
                let pct = if tot > 0 {
                    (cur * 100 / tot).clamp(0, 100) as usize
                } else {
                    0
                };
                let filled = BAR_WIDTH * pct / 100;
                let bar = format!("|{}{}|", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled),);
                let size_str = format!("{} / {}", human_bytes(cur), human_bytes(tot));
                let color = if pct == 100 {
                    Color::Green
                } else {
                    Color::Cyan
                };
                (bar, pct, size_str, color)
            } else {
                let color = if layer.status.to_lowercase().contains("complete")
                    || layer.status.to_lowercase().contains("exists")
                {
                    Color::Green
                } else {
                    Color::DarkGrey
                };
                let bar = format!("|{}|", "█".repeat(BAR_WIDTH));
                (bar, 100, String::new(), color)
            };

            // short ID (12 chars)
            let short_id = &id[..id.len().min(12)];

            out.queue(SetForegroundColor(Color::DarkGrey)).unwrap();
            out.queue(Print(format!("{short_id}  "))).unwrap();

            out.queue(SetForegroundColor(Color::White)).unwrap();
            out.queue(Print(format!("{:<20}", layer.status))).unwrap();

            out.queue(SetForegroundColor(color)).unwrap();
            out.queue(Print(format!("{bar} {pct:>3}%"))).unwrap();

            if !size_str.is_empty() {
                out.queue(SetForegroundColor(Color::DarkGrey)).unwrap();
                out.queue(Print(format!("  {size_str}"))).unwrap();
            }

            out.queue(ResetColor).unwrap();
            out.queue(Print("\n")).unwrap();
        }

        out.flush().unwrap();
        self.drawn_lines = self.layers.len() as u16;
    }
}

pub fn human_bytes(b: i64) -> String {
    humansize::format_size(
        b.max(0) as u64,
        humansize::FormatSizeOptions::from(humansize::DECIMAL).decimal_places(1),
    )
}

// A ':' only separates a tag when it follows the last '/' (otherwise it is a registry port);
// a digest reference goes to the engine whole, in the tag slot.
fn split_image_ref(image: &str) -> (&str, &str) {
    if let Some((repo, digest)) = image.split_once('@') {
        return (repo, digest);
    }
    match image.rfind(':') {
        Some(i) if !image[i..].contains('/') => (&image[..i], &image[i + 1..]),
        _ => (image, "latest"),
    }
}

impl<'a> ImageApi<'a> {
    async fn pull(&self, image: &str) -> Result<ImageInfo, AnyError> {
        let mut progress = PullProgress::new(image);
        let (repo, tag) = split_image_ref(image);
        let mut image_info = self.client.create_image(
            Some(CreateImageOptions {
                from_image: Some(repo.to_string()),
                tag: Some(tag.to_string()),
                platform: self.backend.platform.to_string(),
                ..Default::default()
            }),
            None,
            None,
        );

        while let Some(l) = image_info.next().await {
            match l {
                Ok(CreateImageInfo {
                    id,
                    status,
                    progress_detail,
                    ..
                }) => {
                    if let (Some(id), Some(status)) = (id, status) {
                        let (cur, tot) = match progress_detail {
                            Some(pd) => (pd.current, pd.total),
                            None => (None, None),
                        };
                        progress.update(&id, &status, cur, tot);
                    }
                }
                Err(Error::DockerStreamError { error }) => {
                    return Err(pull_failed(image, &error).into());
                }
                Err(DockerResponseServerError { message, .. }) => {
                    return Err(pull_failed(image, &message.replace('"', "")).into());
                }
                Err(e) => return Err(pull_failed(image, &e.to_string()).into()),
            };
        }
        progress.finished = true;
        if !progress.quiet {
            println!();
        }

        let response = self.client.inspect_image(image).await?;

        image_info_of(image, response)
    }

    pub async fn exists(&self, image: &str) -> Result<bool, AnyError> {
        Ok(self.client.inspect_image(image).await.is_ok())
    }

    // Images rooz builds live under a predictable name in the engine's shared image namespace,
    // which anyone with engine access can tag - and label however they like, so labels alone
    // prove nothing either. Reuse is therefore pinned to the id rooz recorded when it committed
    // the image: an id is the image's content, so it cannot be made to point at somebody else's
    // image. The labels are checked too, to catch an id that is no longer what rooz built.
    // Anything else counts as absent and the image gets built again.
    pub async fn is_pinned(&self, image_id: &str, expected: &Labels) -> Result<bool, AnyError> {
        let refuse = |reason: &str| {
            log::debug!("Not reusing image {}: {}", image_id, reason);
            Ok(false)
        };

        let inspect = match self.client.inspect_image(image_id).await {
            Ok(inspect) => inspect,
            Err(_) => return refuse("no such image"),
        };

        // inspect resolves names as well as ids, so the id it reports has to be the pinned one
        match &inspect.id {
            Some(id) if digest_eq(id, image_id) => (),
            _ => return refuse("the id does not resolve to itself"),
        }

        let found: HashMap<String, String> = inspect
            .config
            .and_then(|c| c.labels)
            .unwrap_or_default()
            .into_iter()
            .collect();

        if !expected.is_subset_of(&found) {
            return refuse("it was not committed by rooz for this workspace");
        }

        Ok(true)
    }

    pub async fn ensure(&self, image: &str, always_pull: bool) -> Result<ImageInfo, AnyError> {
        log::debug!("Ensuring image: {}", image);

        let info = match self.client.inspect_image(image).await {
            Ok(ImageInspect {
                id,
                architecture,
                os,
                ..
            }) => {
                if always_pull && !image.starts_with("localhost/") {
                    self.pull(image).await?
                } else {
                    image_info_of(
                        image,
                        ImageInspect {
                            id,
                            architecture,
                            os,
                            ..Default::default()
                        },
                    )?
                }
            }
            Err(DockerResponseServerError {
                status_code: 404, ..
            }) => self.pull(image).await?,
            Err(e) => {
                return Err(format!(
                    "Could not inspect image '{}': {}. Check that the engine is running.",
                    image, e
                )
                .into());
            }
        };

        log::debug!("Image ID: {:?}", info);
        Ok(info)
    }

    pub async fn get_all(&self, labels: &Labels) -> Result<Vec<ImageSummary>, AnyError> {
        let list_options = ListImagesOptions {
            filters: Some(labels.clone().into()),
            all: true,
            ..Default::default()
        };

        Ok(self.client.list_images(Some(list_options)).await?)
    }

    pub async fn remove_local(&self, image_name: &str, force: bool) -> Result<(), AnyError> {
        let force_display = if force { " (force)" } else { "" };
        match self
            .client
            .remove_image(
                image_name,
                Some(RemoveImageOptions {
                    force,
                    ..Default::default()
                }),
                None,
            )
            .await
        {
            Ok(_) => {
                log::debug!("Removed image: {}{}", image_name, force_display);
                Ok(())
            }
            Err(DockerResponseServerError {
                status_code: 404, ..
            }) => {
                log::debug!("No such image. Skipping: {}{}", image_name, force_display);
                Ok(())
            }
            Err(DockerResponseServerError {
                status_code,
                message,
            }) => Err(format!(
                "{} (Error code: {})",
                message.replace("\"", ""),
                status_code
            )
            .into()),
            Err(e) => Err(format!("Could not remove image '{}': {}", image_name, e).into()),
        }
    }
}

fn pull_failed(image: &str, cause: &str) -> String {
    format!(
        "Pulling image '{}' failed: {}. Check the image name and tag, registry access and network; \
         use --no-pull if a local copy is enough.",
        image,
        cause.trim()
    )
}

fn image_info_of(image: &str, inspect: ImageInspect) -> Result<ImageInfo, AnyError> {
    match (inspect.id, inspect.os, inspect.architecture) {
        (Some(id), Some(os), Some(arch)) => Ok(ImageInfo {
            id,
            platform: Some(format!("{}/{}", os, arch)),
        }),
        _ => Err(format!(
            "Image '{}' reports no id, os or architecture. Remove it and pull again.",
            image
        )
        .into()),
    }
}

// Engines are inconsistent about the `sha256:` prefix: podman's commit returns the bare digest
// while inspect reports the prefixed form.
fn digest_eq(a: &str, b: &str) -> bool {
    fn bare(v: &str) -> &str {
        v.strip_prefix("sha256:").unwrap_or(v)
    }
    !bare(a).is_empty() && bare(a) == bare(b)
}

#[cfg(test)]
mod tests {
    use super::add_layer_bytes;
    use indexmap::IndexMap;

    #[test]
    fn layer_bytes_add_up_to_the_images_progress() {
        let mut bytes = IndexMap::new();
        add_layer_bytes(&mut bytes, "a", "Downloading", Some(10), Some(100));
        let (cur, tot) = add_layer_bytes(&mut bytes, "b", "Downloading", Some(40), Some(200));
        assert_eq!((cur, tot), (50, 300));
        // a finished layer counts whole, an unrelated status changes nothing
        add_layer_bytes(&mut bytes, "a", "Download complete", None, None);
        let (cur, tot) = add_layer_bytes(&mut bytes, "b", "Waiting", None, None);
        assert_eq!((cur, tot), (140, 300));
        let (cur, tot) = add_layer_bytes(&mut bytes, "b", "Pull complete", None, None);
        assert_eq!((cur, tot), (300, 300));
        // layers already there have no size and add nothing
        let (cur, tot) = add_layer_bytes(&mut bytes, "c", "Already exists", None, None);
        assert_eq!((cur, tot), (300, 300));
    }

    use super::split_image_ref;

    #[test]
    fn image_refs_split_on_the_tag_colon_only() {
        for (image, want) in [
            ("alpine", ("alpine", "latest")),
            ("alpine:3", ("alpine", "3")),
            ("images:5000/alpine", ("images:5000/alpine", "latest")),
            ("images:5000/alpine:3", ("images:5000/alpine", "3")),
            ("img@sha256:abc", ("img", "sha256:abc")),
        ] {
            assert_eq!(split_image_ref(image), want, "{}", image);
        }
    }

    use super::*;
    use crate::util::labels;

    #[test]
    fn sizes_are_decimal_with_one_decimal_place() {
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(12_658_146), "12.7 MB");
        assert_eq!(human_bytes(-5), "0 B");
    }

    #[test]
    fn pull_failure_names_image_cause_and_fix() {
        let m = pull_failed("nope:1", "manifest unknown\n");
        assert!(m.contains("nope:1") && m.contains("manifest unknown") && m.contains("--no-pull"));
    }

    #[test]
    fn incomplete_inspect_is_an_error_not_a_panic() {
        let err = image_info_of("x", ImageInspect::default()).unwrap_err();
        assert!(err.to_string().contains("'x'"));
        let ok = image_info_of(
            "x",
            ImageInspect {
                id: Some("sha256:a".into()),
                os: Some("linux".into()),
                architecture: Some("amd64".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(ok.platform.as_deref(), Some("linux/amd64"));
    }

    fn expected() -> Labels {
        Labels::from(&[
            Labels::workspace("ws"),
            Labels::role(labels::SIDECAR_RUNTIME_ROLE),
            Labels::container("svc"),
        ])
    }

    fn found(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn an_image_rooz_committed_for_this_sidecar_is_reused() {
        let mine: HashMap<String, String> = expected().into();
        assert!(expected().is_subset_of(&mine));
        // extra labels on the image are fine
        let mut extra = mine.clone();
        extra.insert("org.opencontainers.created".into(), "yesterday".into());
        assert!(expected().is_subset_of(&extra));
    }

    #[test]
    fn a_squatted_tag_is_not_reused() {
        // no labels at all - the plain `docker tag` case from the report
        assert!(!expected().is_subset_of(&found(&[])));
        // forged rooz labels naming a different workspace or sidecar
        assert!(!expected().is_subset_of(&found(&[
            (labels::ROOZ, "true"),
            (labels::WORKSPACE_KEY, "someone-else"),
            (labels::ROLE, labels::SIDECAR_RUNTIME_ROLE),
            (labels::CONTAINER, "svc"),
        ])));
        assert!(!expected().is_subset_of(&found(&[
            (labels::ROOZ, "true"),
            (labels::WORKSPACE_KEY, "ws"),
            (labels::ROLE, labels::SIDECAR_RUNTIME_ROLE),
            (labels::CONTAINER, "other-sidecar"),
        ])));
    }

    #[test]
    fn digests_compare_across_the_sha256_prefix() {
        assert!(digest_eq("sha256:abc123", "abc123"));
        assert!(digest_eq("abc123", "sha256:abc123"));
        assert!(digest_eq("sha256:abc123", "sha256:abc123"));
        assert!(!digest_eq("sha256:abc123", "sha256:def456"));
        // a tag is not a digest: an empty or prefix-only value must never match
        assert!(!digest_eq("sha256:", "sha256:"));
        assert!(!digest_eq("", ""));
        assert!(!digest_eq("localhost/rooz/ws/svc:latest", "sha256:abc123"));
    }
}
