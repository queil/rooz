use crate::api::container::inject;
use crate::api::CryptApi;
use crate::model::types::{AnyError, ContainerResult, RunSpec};
use crate::{constants, util::id};
use age::x25519::{Identity, Recipient};
use age::IdentityFileEntry::Native;
use bollard::models::MountTypeEnum::VOLUME;
use bollard::service::Mount;
use std::io::{Read, Write};
use std::iter;

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
        let entrypoint = inject(&format!("cat {}/age.key", work_dir), "entrypoint.sh");
        let run_spec = RunSpec {
            reason: "read-age-key",
            image: constants::DEFAULT_IMAGE,
            uid: constants::ROOT_UID,
            work_dir: None,
            container_name: &id::random_suffix("read-age"),
            workspace_key: &id::random_suffix("tmp"),
            mounts: Some(vec![self.mount(work_dir)]),
            entrypoint: Some(vec!["cat"]),
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
                let data = self
                    .api
                    .exec
                    .output(
                        "read age key",
                        container_id,
                        None,
                        Some(entrypoint.iter().map(String::as_str).collect()),
                    )
                    .await?;
                self.api.container.kill(&container_id).await?;

                let identity_file = age::IdentityFile::from_buffer(data.as_bytes())?;
                match identity_file.into_identities().first().unwrap() {
                    Native(id2) => Ok(id2.clone()),
                }
            }
            _ => panic!("Could not read age identity"),
        }
    }

    pub fn encrypt(&self, plaintext: String, recipient: Recipient) -> Result<String, AnyError> {
        let encryptor = age::Encryptor::with_recipients(vec![Box::new(recipient)]).unwrap();
        let mut encrypted = vec![];
        let mut writer = encryptor.wrap_output(age::armor::ArmoredWriter::wrap_output(
            &mut encrypted,
            age::armor::Format::AsciiArmor,
        )?)?;
        writer.write_all(plaintext.as_bytes())?;
        writer.finish().and_then(|armor| armor.finish())?;
        Ok(std::str::from_utf8(&encrypted)?
            .to_string()
            .replace("\n", "|"))
    }

    //TODO: improve experience when there is no matching decryption key
    pub fn decrypt(&self, identity: &dyn age::Identity, secret: &str) -> Result<String, AnyError> {
        let formatted = secret.replace("|", "\n");
        let encrypted = formatted.as_bytes();
        let decrypted = {
            let decryptor = match age::Decryptor::new(age::armor::ArmoredReader::new(encrypted))? {
                age::Decryptor::Recipients(d) => d,
                _ => unreachable!(),
            };

            let mut decrypted = vec![];
            let mut reader = decryptor.decrypt(iter::once(identity))?;
            reader.read_to_end(&mut decrypted)?;
            decrypted
        };
        Ok(std::str::from_utf8(&decrypted[..])?.to_string())
    }
}
