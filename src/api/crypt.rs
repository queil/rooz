use crate::api::container::inject;
use crate::api::CryptApi;
use crate::model::types::{AnyError, ContainerResult, RunSpec};
use crate::{constants, util::id};
use age::x25519::Identity;
use bollard::models::MountTypeEnum::VOLUME;
use bollard::service::Mount;
use std::str::FromStr;

pub const VOLUME_NAME: &'static str = "rooz-age-key-vol";

impl<'a> CryptApi<'a> {
    pub fn mount(&self, target: &str) -> Mount {
        Mount {
            typ: Some(VOLUME),
            source: Some(VOLUME_NAME.into()),
            target: Some(target.into()),
            ..Default::default()
        }
    }

    pub async fn read_age_identity(&self) -> Result<Identity, AnyError> {
        let work_dir = "/tmp/.age";

        let run_spec = RunSpec {
            reason: "read-age-key",
            image: constants::DEFAULT_IMAGE,
            uid: constants::ROOT_UID_INT,
            work_dir: None,
            container_name: &id::random_suffix("read-age"),
            workspace_key: &id::random_suffix("tmp"),
            mounts: Some(vec![self.mount(work_dir)]),
            entrypoint: constants::default_entrypoint(),
            privileged: false,
            force_recreate: false,
            auto_remove: true,
            ..Default::default()
        };

        let container_result = self.api.container.create(run_spec).await?;
        self.api.container.start(container_result.id()).await?;
        let container_id = container_result.id();

        match container_result {
            ContainerResult::Created { .. } => {
                let command = inject(&format!("cat {}/age.key", work_dir), "entrypoint.sh");
                let data = self
                    .api
                    .exec
                    .output(
                        "read age key",
                        container_id,
                        None,
                        Some(command.iter().map(String::as_str).collect()),
                    )
                    .await?;
                self.api.container.kill(&container_id).await?;

                Ok(age::x25519::Identity::from_str(&data)?)
            }
            _ => panic!("Could not read age identity"),
        }
    }

    pub fn encrypt(
        &self,
        plaintext: String,
        recipient: &impl age::Recipient,
    ) -> Result<String, AnyError> {
        Ok(
            age::encrypt_and_armor(recipient, plaintext.into_bytes().as_slice())?
                .replace("\n", "|"),
        )
    }

    //TODO: improve experience when there is no matching decryption key
    pub fn decrypt(&self, identity: &Identity, secret: &str) -> Result<String, AnyError> {
        let formatted = secret.replace("|", "\n");
        let ciphertext = formatted.as_bytes();
        Ok(std::str::from_utf8(age::decrypt(identity, ciphertext)?.as_slice())?.to_string())
    }
}
