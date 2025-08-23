use crate::api::CryptApi;
use crate::config::config::SystemConfig;
use crate::model::types::AnyError;
use age::x25519::Identity;
use bollard::models::MountTypeEnum::VOLUME;
use bollard::service::Mount;
use std::str::FromStr;

pub const VOLUME_NAME: &'static str = "rooz-age-key-vol";

impl SystemConfig {
    pub fn age_identity(&self) -> Result<Identity, AnyError> {
        Ok(age::x25519::Identity::from_str(
            self.age_key.as_deref().unwrap(),
        )?)
    }
}

impl CryptApi {
    pub fn mount(&self, target: &str) -> Mount {
        Mount {
            typ: Some(VOLUME),
            source: Some(VOLUME_NAME.into()),
            target: Some(target.into()),
            ..Default::default()
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
