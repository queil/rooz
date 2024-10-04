use super::config::RoozCfg;
use crate::{model::types::AnyError, util::crypt};
use age::x25519::Identity;
use linked_hash_map::LinkedHashMap;

impl<'a> RoozCfg {
    pub async fn decrypt(&mut self, identity: &Identity) -> Result<(), AnyError> {
        self.secrets = match self.secrets.clone() {
            Some(secrets) if secrets.len() > 0 => {
                log::debug!("Decrypting secrets");
                let mut ret = LinkedHashMap::<String, String>::new();
                for (k, v) in secrets.iter() {
                    ret.insert(k.to_string(), crypt::decrypt(identity, v)?);
                }
                Some(ret)
            }
            Some(empty) => Some(empty),
            None => None,
        };
        Ok(())
    }

    pub async fn encrypt(&mut self, identity: &Identity) -> Result<(), AnyError> {
        let mut encrypted_secrets = LinkedHashMap::<String, String>::new();
        if let Some(edited_secrets) = self.clone().secrets {
            for (k, v) in edited_secrets {
                encrypted_secrets.insert(
                    k.to_string(),
                    crypt::encrypt(v.to_string(), identity.to_public())?,
                );
            }
        };
        self.secrets = if encrypted_secrets.len() > 0 {
            Some(encrypted_secrets)
        } else {
            None
        };
        Ok(())
    }
}
