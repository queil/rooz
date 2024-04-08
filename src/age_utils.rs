use crate::api::{ContainerApi, ExecApi};
use crate::labels::KeyValue;
use crate::model::types::AnyError;
use age::x25519::{Identity, Recipient};
use age::IdentityFileEntry::Native;
use bollard::models::MountTypeEnum::VOLUME;
use bollard::service::Mount;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::iter;
use base64::{engine::general_purpose, Engine as _};

pub const VOLUME_NAME: &'static str = "rooz-age-key-vol";
const AGE_HEADER: &'static str = "age-encryption.org/v1";

pub fn mount(target: &str) -> Mount {
    Mount {
        typ: Some(VOLUME),
        source: Some(VOLUME_NAME.into()),
        target: Some(target.into()),
        ..Default::default()
    }
}

impl<'a> ContainerApi<'a> {
    pub async fn read_age_identity(&self) -> Result<Identity, AnyError> {
        
        Ok(age::x25519::Identity::generate())

        // let data = "TODO: READ FROM CONTAINER".as_bytes();

        // let identity_file = age::IdentityFile::from_buffer(data)?;
        // match identity_file.into_identities().first().unwrap() {
        //     Native(id2) => Ok(id2)
        // }
    }
}

pub fn needs_decryption(env_vars: Option<HashMap<String,String>>) -> Option<HashMap<String,String>> {
    if let Some(vars) = env_vars {
        if vars.iter().any(|(_,v)| v.starts_with(AGE_HEADER)) {
            Some(vars)
        }else
        {
            None
        }
    }
    else {
        None
    }
}

pub fn encrypt(plaintext: String, recipient: Recipient) -> Result<String, AnyError> {
    let encryptor = age::Encryptor::with_recipients(vec![Box::new(recipient)]).unwrap();
    let mut encrypted = vec![];
    let mut writer = encryptor.wrap_output(&mut encrypted)?;
    writer.write_all(plaintext.as_bytes())?;
    writer.finish()?;
    Ok(general_purpose::STANDARD.encode(&encrypted))
}

pub fn decrypt(
    identity: &dyn age::Identity,
    env_vars: HashMap<String,String>,
) -> Result<HashMap<String,String>, AnyError> {
   
        let mut ret = HashMap::<String,String>::new();
        for (k,v) in env_vars.iter() {

            let decoded_vec = &general_purpose::STANDARD.decode(&v).unwrap();
            let decoded = std::str::from_utf8(decoded_vec)?;
            if decoded.starts_with(AGE_HEADER) {
                let decrypted = {
                    let decryptor = match age::Decryptor::new(&decoded.as_bytes()[..])? {
                        age::Decryptor::Recipients(d) => d,
                        _ => unreachable!(),
                    };

                    let mut decrypted = vec![];
                    let mut reader = decryptor.decrypt(iter::once(identity))?;
                    reader.read_to_end(&mut decrypted)?;

                    decrypted
                };

                ret.insert(k.to_string(), std::str::from_utf8(&decrypted[..])?.to_string());
            }
        }
        Ok(ret)
}
