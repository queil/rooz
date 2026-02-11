use crate::{api::ImageApi, model::types::AnyError};
use bollard::errors::Error::DockerResponseServerError;
use bollard::models::CreateImageInfo;
use bollard::service::ImageInspect;
use bollard::{errors::Error, query_parameters::CreateImageOptions};
use futures::StreamExt;
use std::io::{Write, stdout};

#[derive(Debug)]
pub struct ImageInfo {
    pub id: String,
    pub platform: Option<String>,
}

impl<'a> ImageApi<'a> {
    async fn pull(&self, image: &str) -> Result<ImageInfo, AnyError> {
        println!("Pulling image: {}", &image);
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
                    status: Some(m),
                    progress: p,
                    ..
                }) => {
                    if let Some(id) = id {
                        stdout().write_all(&id.as_bytes())?;
                    } else {
                        println!("");
                    }
                    print!(" ");
                    stdout().write_all(&m.as_bytes())?;
                    print!(" ");
                    if let Some(x) = p {
                        stdout().write_all(&x.as_bytes())?;
                    };
                    print!("\r");
                }
                Ok(msg) => panic!("{:?}", msg),
                Err(Error::DockerStreamError { error }) => eprintln!("{}", error),
                e => panic!("{:?}", e),
            };
        }
        println!("");

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

    pub async fn ensure(&self, image: &str, always_pull: bool) -> Result<ImageInfo, AnyError> {
        log::debug!("Ensuring image: {}", &image);

        let info = match self.client.inspect_image(&image).await {
            Ok(ImageInspect {
                id,
                architecture,
                os,
                ..
            }) => {
                if always_pull {
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
}
