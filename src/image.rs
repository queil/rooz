use crate::backend::ImageApi;
use bollard::errors::Error;
use bollard::errors::Error::DockerResponseServerError;
use bollard::image::CreateImageOptions;
use bollard::models::CreateImageInfo;
use bollard::service::ImageInspect;
use futures::StreamExt;
use std::io::{stdout, Write};

impl<'a> ImageApi<'a> {
    async fn pull(
        &self,
        image: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error + 'static>> {
        println!("Pulling image: {}", &image);
        let img_chunks = &image.split(':').collect::<Vec<&str>>();
        let mut image_info = self.client.create_image(
            Some(CreateImageOptions::<&str> {
                from_image: img_chunks[0],
                tag: match img_chunks.len() {
                    2 => img_chunks[1],
                    _ => "latest",
                },
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
        Ok(self.client.inspect_image(&image).await?.id)
    }

    pub async fn ensure(
        &self,
        image: &str,
        always_pull: bool,
    ) -> Result<String, Box<dyn std::error::Error + 'static>> {
        let image_id = match self.client.inspect_image(&image).await {
            Ok(ImageInspect { id, .. }) => {
                if always_pull {
                    self.pull(image).await?
                } else {
                    id
                }
            }
            Err(DockerResponseServerError {
                status_code: 404, ..
            }) => self.pull(image).await?,
            Err(e) => panic!("{:?}", e),
        };

        log::debug!("Image ID: {:?}", image_id);
        Ok(image_id.unwrap())
    }
}
