use super::config::RoozCfg;
use crate::{api::ConfigApi, model::types::AnyError};
use age::x25519::Identity;
use linked_hash_map::LinkedHashMap;

impl<'a> ConfigApi<'a> {
    pub async fn decrypt(&self, config: &mut RoozCfg, identity: &Identity) -> Result<(), AnyError> {
        config.secrets = match config.secrets.clone() {
            Some(secrets) if secrets.len() > 0 => {
                log::debug!("Decrypting secrets");
                let mut ret = LinkedHashMap::<String, String>::new();
                for (k, v) in secrets.iter() {
                    ret.insert(k.to_string(), self.crypt.decrypt(identity, v)?);
                }
                Some(ret)
            }
            Some(empty) => Some(empty),
            None => None,
        };
        Ok(())
    }

    pub async fn encrypt(&self, config: &mut RoozCfg, identity: &Identity) -> Result<(), AnyError> {
        let mut encrypted_secrets = LinkedHashMap::<String, String>::new();
        if let Some(edited_secrets) = config.clone().secrets {
            for (k, v) in edited_secrets {
                encrypted_secrets.insert(
                    k.to_string(),
                    self.crypt.encrypt(v.to_string(), identity.to_public())?,
                );
            }
        };
        config.secrets = if encrypted_secrets.len() > 0 {
            Some(encrypted_secrets)
        } else {
            None
        };
        Ok(())
    }
}
