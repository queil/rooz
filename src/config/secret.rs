use super::config::RoozCfg;
use crate::{age_utils, model::types::AnyError};
use age::x25519::Identity;

impl<'a> RoozCfg {
    pub async fn decrypt(&mut self, identity: Identity) -> Result<(), AnyError> {
        self.secrets = match self.secrets.clone() {
            Some(secrets) if secrets.len() > 0 => {
                log::debug!("Decrypting secrets");
                Some(age_utils::decrypt(&identity, secrets)?)
            }
            Some(empty) => Some(empty),
            None => None,
        };
        Ok(())
    }
}
