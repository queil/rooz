use crate::api::CryptApi;
use crate::config::config::SystemConfig;
use crate::model::types::AnyError;
use age::x25519::Identity;
use std::str::FromStr;

impl SystemConfig {
    pub fn age_identity(&self) -> Result<Identity, AnyError> {
        let key = self.age_key.as_deref().ok_or(format!(
            "No age identity found at {:?}. Run 'rooz system init' to create one, or point \
             {} at an existing identity file.",
            crate::util::identity::key_path()?,
            crate::util::identity::AGE_KEY_FILE_ENV
        ))?;
        Ok(Identity::from_str(key)?)
    }
}

impl CryptApi {
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
