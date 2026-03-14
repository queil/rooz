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
use std::io::{Write, stdout};

#[derive(Debug)]
pub struct ImageInfo {
    pub id: String,
    pub platform: Option<String>,
}

const BAR_WIDTH: usize = 20;

struct LayerState {
    status: String,
    current: Option<i64>,
    total: Option<i64>,
}

pub struct PullProgress {
    layers: IndexMap<String, LayerState>,
    drawn_lines: u16,
}

impl PullProgress {
    pub fn new(image: &str) -> Self {
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
            layers: IndexMap::new(),
            drawn_lines: 0,
        }
    }

    pub fn update(&mut self, id: &str, status: &str, current: Option<i64>, total: Option<i64>) {
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
                    (cur * 100 / tot) as usize
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

fn human_bytes(b: i64) -> String {
    const MB: f64 = 1_000_000.0;
    const KB: f64 = 1_000.0;
    let b = b as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{b} B")
    }
}
impl<'a> ImageApi<'a> {
    async fn pull(&self, image: &str) -> Result<ImageInfo, AnyError> {
        let mut progress = PullProgress::new(&image);
        let img_chunks = &image.split(':').collect::<Vec<&str>>();
        let mut image_info = self.client.create_image(
            Some(CreateImageOptions {
                from_image: Some(img_chunks[0].to_string()),
                tag: Some(
                    match img_chunks.len() {
                        2 => img_chunks[1],
                        _ => "latest",
                    }
                    .to_string(),
                ),
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
                    status: Some(status),
                    progress_detail: p,
                    ..
                }) => {
                    if let (Some(id), status) = (&id, &status) {
                        progress.update(id, status, p.clone().unwrap().current, p.unwrap().total);
                    }
                }
                Ok(msg) => panic!("{:?}", msg),
                Err(Error::DockerStreamError { error }) => eprintln!("{}", error),
                e => panic!("{:?}", e),
            };
        }
        println!();

        let response = self.client.inspect_image(&image).await?;

        Ok(ImageInfo {
            id: response.id.unwrap(),
            platform: Some(format!(
                "{}/{}",
                response.os.unwrap(),
                response.architecture.unwrap()
            )),
        })
    }

    pub async fn exists(&self, image: &str) -> Result<bool, AnyError> {
        Ok(match self.client.inspect_image(&image).await {
            Ok(_) => true,
            Err(_) => false,
        })
    }

    pub async fn ensure(&self, image: &str, always_pull: bool) -> Result<ImageInfo, AnyError> {
        log::debug!("Ensuring image: {}", &image);

        let info = match self.client.inspect_image(&image).await {
            Ok(ImageInspect {
                id,
                architecture,
                os,
                ..
            }) => {
                if always_pull && !image.starts_with("localhost/") {
                    self.pull(image).await?
                } else {
                    ImageInfo {
                        id: id.unwrap(),
                        platform: Some(format!("{}/{}", os.unwrap(), architecture.unwrap())),
                    }
                }
            }
            Err(DockerResponseServerError {
                status_code: 404, ..
            }) => self.pull(image).await?,
            Err(e) => panic!("{:?}", e),
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
                &image_name,
                Some(RemoveImageOptions {
                    force,
                    ..Default::default()
                }),
                None,
            )
            .await
        {
            Ok(_) => {
                log::debug!("Removed image: {}{}", &image_name, &force_display);
                Ok(())
            }
            Err(DockerResponseServerError {
                status_code: 404, ..
            }) => {
                log::debug!("No such image. Skipping: {}{}", &image_name, &force_display);
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
            Err(e) => panic!("{}", e),
        }
    }
}
