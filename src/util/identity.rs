use std::path::PathBuf;

use crate::model::types::AnyError;

pub const AGE_KEY_FILE_ENV: &str = "ROOZ_AGE_KEY_FILE";

// The age identity decrypts every secret the operator ever encrypted with rooz, including
// the ciphertext they commit to repositories, so it lives on the operator's machine and
// never on the engine. Nothing in a container needs it: only the CLI decrypts (create,
// update, config edit) and encrypts (config edit). A container engine hands every one of
// its API users the contents of every named volume, so an identity kept in one is shared
// with everybody who can reach that engine.
pub fn key_path() -> Result<PathBuf, AnyError> {
    if let Some(path) = std::env::var(AGE_KEY_FILE_ENV)
        .ok()
        .filter(|p| !p.trim().is_empty())
    {
        return Ok(PathBuf::from(path));
    }
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .filter(|p| !p.trim().is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .ok_or(format!(
            "Could not work out where to keep the age identity: set HOME, XDG_CONFIG_HOME or {}",
            AGE_KEY_FILE_ENV
        ))?;
    Ok(config_dir.join("rooz").join("age.key"))
}

pub fn read() -> Result<Option<String>, AnyError> {
    let path = key_path()?;
    match std::fs::read_to_string(&path) {
        Ok(key) if !key.trim().is_empty() => Ok(Some(key.trim().to_string())),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Could not read the age identity at {:?}: {}", path, e).into()),
    }
}

pub fn write(key: &str) -> Result<PathBuf, AnyError> {
    let path = key_path()?;
    write_private(&path, key).map_err(|e| {
        format!(
            "Could not write the age identity to {:?}: {}. Point {} somewhere writable if that \
             location is not yours to write.",
            path, e, AGE_KEY_FILE_ENV
        )
    })?;
    Ok(path)
}

// Keeps a copy that would otherwise be lost, without overwriting an existing one.
pub fn back_up(key: &str) -> Result<PathBuf, AnyError> {
    let path = key_path()?.with_extension("engine.bak");
    if path.exists() {
        return Err(format!(
            "Refusing to overwrite the existing backup at {:?} - move it aside first",
            path
        )
        .into());
    }
    write_private(&path, key)
        .map_err(|e| format!("Could not write the identity backup to {:?}: {}", path, e))?;
    Ok(path)
}

fn write_private(path: &PathBuf, key: &str) -> Result<(), std::io::Error> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("{}\n", key.trim()))?;
    set_private(path)
}

#[cfg(unix)]
fn set_private(path: &PathBuf) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private(_path: &PathBuf) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // the env is process-global, so the cases that set it share one test
    #[test]
    fn key_path_prefers_the_explicit_override_then_xdg_then_home() {
        unsafe {
            std::env::set_var(AGE_KEY_FILE_ENV, "/keys/rooz.key");
            std::env::set_var("XDG_CONFIG_HOME", "/xdg");
            std::env::set_var("HOME", "/home/op");
        }
        assert_eq!(key_path().unwrap(), PathBuf::from("/keys/rooz.key"));

        unsafe { std::env::remove_var(AGE_KEY_FILE_ENV) };
        assert_eq!(key_path().unwrap(), PathBuf::from("/xdg/rooz/age.key"));

        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
        assert_eq!(
            key_path().unwrap(),
            PathBuf::from("/home/op/.config/rooz/age.key")
        );
    }
}
