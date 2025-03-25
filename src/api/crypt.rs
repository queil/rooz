use crate::api::CryptApi;
use crate::model::types::AnyError;
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

        let result = self
            .api
            .container
            .one_shot_output(
                "read-age-key",
                "cat /tmp/.age/age.key".into(),
                Some(vec![self.mount(work_dir)]),
            )
            .await?;
        Ok(age::x25519::Identity::from_str(&result.data)?)
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
