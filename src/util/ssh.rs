use bollard::models::MountType::VOLUME;
use bollard::service::Mount;

pub const VOLUME_NAME: &'static str = "rooz-ssh-key-vol";

// The key volume is global and persistent: one key pair for the whole engine, never
// regenerated. Containers built from a repository's configuration get it read-only so a
// hostile workspace cannot overwrite or delete the operator's git identity. Only rooz's
// own fixed-image containers (key generation, and cloning, which records known_hosts)
// mount it writable.
pub fn mount(target: &str, read_only: bool) -> Mount {
    Mount {
        typ: Some(VOLUME),
        source: Some(VOLUME_NAME.into()),
        target: Some(target.into()),
        read_only: Some(read_only),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{VOLUME_NAME, mount};

    #[test]
    fn workspace_mount_is_read_only() {
        let m = mount("/home/rooz_user/.ssh", true);
        assert_eq!(m.source.as_deref(), Some(VOLUME_NAME));
        assert_eq!(m.target.as_deref(), Some("/home/rooz_user/.ssh"));
        assert_eq!(m.read_only, Some(true));
    }

    #[test]
    fn writable_mount_is_explicit() {
        assert_eq!(mount("/tmp/.ssh", false).read_only, Some(false));
    }
}
