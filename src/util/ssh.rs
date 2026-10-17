use bollard::models::MountType::VOLUME;
use bollard::service::Mount;

pub const VOLUME_NAME: &str = "rooz_ssh-key";

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

// The workspace mounts the key volume read-only, so ssh cannot rewrite known_hosts there.
// With the default UpdateHostKeys it tries after every connection where the server offers
// more host keys, and complains ("hostfile_replace_entries: mkstemp: Read-only file system")
// on each git pull/fetch. This client config, kept in the volume, tells ssh not to try.
// Rooz's own writable containers keep working: they record known_hosts at clone time.
pub fn ensure_config_script(dir: &str) -> String {
    format!(
        "[ -e {dir}/config ] || printf 'Host *\\n  UpdateHostKeys no\\n' > {dir}/config\nchmod 600 {dir}/config\n",
        dir = dir
    )
}

#[cfg(test)]
mod tests {
    use super::{VOLUME_NAME, ensure_config_script, mount};

    #[test]
    fn config_script_is_idempotent_and_disables_host_key_updates() {
        let script = ensure_config_script("/tmp/.ssh");
        assert!(script.starts_with("[ -e /tmp/.ssh/config ] || "));
        assert!(script.contains("UpdateHostKeys no"));
        assert!(script.contains("chmod 600 /tmp/.ssh/config"));
    }

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
