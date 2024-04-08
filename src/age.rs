use std::io::Read;
use std::iter;
use std::str::FromStr;

use bollard::models::MountTypeEnum::VOLUME;
use bollard::service::Mount;

use crate::labels::KeyValue;
use crate::model::types::AnyError;

pub const VOLUME_NAME: &'static str = "rooz-age-key-vol";

pub fn mount(target: &str) -> Mount {
    Mount {
        typ: Some(VOLUME),
        source: Some(VOLUME_NAME.into()),
        target: Some(target.into()),
        ..Default::default()
    }
}

pub fn decrypt(env_vars: Vec<KeyValue>) -> Result<Vec<KeyValue>, AnyError> {
    if env_vars
        .iter()
        .any(|v| v.value.starts_with("age-encryption.org/v1"))
    {
        let mut ret = Vec::<KeyValue>::new();
        for KeyValue { key, value, .. } in env_vars.iter() {
            if value.starts_with("age-encryption.org/v1") {
                let age_key = age::x25519::Identity::from_str(
                    "LOAD_THIS_FROM_CONTAINER"
                )?;

                let decrypted = {
                    let decryptor = match age::Decryptor::new(&value.as_bytes()[..])? {
                        age::Decryptor::Recipients(d) => d,
                        _ => unreachable!(),
                    };

                    let mut decrypted = vec![];
                    let mut reader =
                        decryptor.decrypt(iter::once(&age_key as &dyn age::Identity))?;
                    reader.read_to_end(&mut decrypted)?;

                    decrypted
                };

                ret.push(KeyValue::new(key, std::str::from_utf8(&decrypted[..])?))
            }
        }
        Ok(ret)
    } else {
        Ok(env_vars)
    }
}
