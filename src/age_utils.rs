use crate::api::container::inject;
use crate::api::WorkspaceApi;
use crate::labels::Labels;
use crate::model::types::{AnyError, ContainerResult, RunSpec};
use crate::{constants, id};
use age::x25519::{Identity, Recipient};
use age::IdentityFileEntry::Native;
use bollard::models::MountTypeEnum::VOLUME;
use bollard::service::Mount;
use linked_hash_map::LinkedHashMap;
use std::io::{Read, Write};
use std::iter;

pub const VOLUME_NAME: &'static str = "rooz-age-key-vol";
const SECRET_HEADER: &'static str = "-----BEGIN AGE ENCRYPTED FILE-----";

pub fn mount(target: &str) -> Mount {
    Mount {
        typ: Some(VOLUME),
        source: Some(VOLUME_NAME.into()),
        target: Some(target.into()),
        ..Default::default()
    }
}

impl<'a> WorkspaceApi<'a> {
    pub async fn read_age_identity(&self) -> Result<Identity, AnyError> {
        let workspace_key = id::random_suffix("tmp");
        let labels = Labels::default();
        let work_dir = "/tmp/.age";
        let entrypoint = inject(&format!("cat {}/age.key", work_dir), "entrypoint.sh");
        let run_spec = RunSpec {
            reason: "read-age-key",
            image: constants::DEFAULT_IMAGE,
            uid: constants::ROOT_UID,
            work_dir: None,
            container_name: &id::random_suffix("read-age"),
            workspace_key: &workspace_key,
            mounts: Some(vec![mount(work_dir)]),
            entrypoint: Some(vec!["cat"]),
            privileged: false,
            force_recreate: false,
            auto_remove: true,
            labels,
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
}

pub fn needs_decryption(
    env_vars: Option<LinkedHashMap<String, String>>,
) -> Option<LinkedHashMap<String, String>> {
    if let Some(vars) = env_vars {
        if vars.iter().any(|(_, v)| v.starts_with(SECRET_HEADER)) {
            Some(vars)
        } else {
            None
        }
    } else {
        None
    }
}

pub fn encrypt(plaintext: String, recipient: Recipient) -> Result<String, AnyError> {
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

pub fn decrypt(
    identity: &dyn age::Identity,
    env_vars: LinkedHashMap<String, String>,
) -> Result<LinkedHashMap<String, String>, AnyError> {
    let mut ret = LinkedHashMap::<String, String>::new();
    for (k, v) in env_vars.iter() {
        if v.starts_with(SECRET_HEADER) {
            let formatted = v.replace("|", "\n");
            let encrypted = formatted.as_bytes();
            let decrypted = {
                let decryptor =
                    match age::Decryptor::new(age::armor::ArmoredReader::new(encrypted))? {
                        age::Decryptor::Recipients(d) => d,
                        _ => unreachable!(),
                    };

                let mut decrypted = vec![];
                let mut reader = decryptor.decrypt(iter::once(identity))?;
                reader.read_to_end(&mut decrypted)?;

                decrypted
            };

            ret.insert(
                k.to_string(),
                std::str::from_utf8(&decrypted[..])?.to_string(),
            );
        } else {
            ret.insert(k.to_string(), v.to_string());
        }
    }
    Ok(ret)
}
