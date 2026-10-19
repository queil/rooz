pub const DEFAULT_IMAGE: &str = "docker.io/chainguard/git:latest-dev";
pub const DEFAULT_SHELL: &str = "sh";
pub const DEFAULT_USER: &str = "rooz_user";
pub const DEFAULT_UID: &str = "1000";
pub const DEFAULT_CONTAINER_NAME: &str = "work";
pub const ROOT_UID: &str = "0";
pub const ROOT_UID_INT: i32 = 0;
pub const ROOT_USER: &str = "root";
pub const WORK_DIR: &str = "/work";
pub const CONFIG_CLONE_DIR: &str = "/tmp/rooz-config";
pub const SYSTEM_CONFIG_FILE: &str = "rooz.config";
pub const SYSTEM_CONFIG_MOUNT: &str = "/tmp/sys";
pub fn default_command<'a>() -> Option<Vec<&'a str>> {
    Some(vec!["cat"])
}

// Workspace and sidecar names hold only [a-z0-9-], so '_' joins them unambiguously: with '-',
// workspace 'a' with sidecar 'x' and workspace 'a-x' would share a container name, and the pairs
// (a, b-c) and (a-b, c) a peer network.
pub fn sidecar_container(workspace_key: &str, sidecar: &str) -> String {
    format!("{}_{}", workspace_key, sidecar)
}
pub fn egress_network(workspace_key: &str) -> String {
    format!("{}_egress", workspace_key)
}
pub fn pair_network(workspace_key: &str, sidecar: &str) -> String {
    fit_network_name(format!("{}_net_{}", workspace_key, sidecar))
}
pub fn peer_network(workspace_key: &str, a: &str, b: &str) -> String {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    fit_network_name(format!("{}_peer_{}_{}", workspace_key, a, b))
}

const NETWORK_NAME_MAX: usize = 63;

fn fit_network_name(name: String) -> String {
    if name.len() <= NETWORK_NAME_MAX {
        return name;
    }
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in name.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let prefix: String = name.chars().take(NETWORK_NAME_MAX - 17).collect();
    format!("{}-{:016x}", prefix, hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_network_sorts_args() {
        assert_eq!(
            peer_network("ws", "dkr", "images"),
            peer_network("ws", "images", "dkr")
        );
        assert_eq!(peer_network("ws", "a", "b"), "ws_peer_a_b");
    }

    #[test]
    fn names_from_different_parts_never_collide() {
        // what '-' as the separator let collide
        // workspace 'a-x' names its work container 'a-x'
        assert_ne!(sidecar_container("a", "x"), "a-x");
        assert_ne!(
            peer_network("ws", "a", "b-c"),
            peer_network("ws", "a-b", "c")
        );
        assert_ne!(pair_network("c", "net-y"), pair_network("c-net", "y"));
    }

    #[test]
    fn long_network_names_truncated_with_hash() {
        let ws = "w".repeat(50);
        let name = pair_network(&ws, &"s".repeat(50));
        assert_eq!(name.len(), NETWORK_NAME_MAX);
        assert_eq!(name, pair_network(&ws, &"s".repeat(50)));
        assert_ne!(name, pair_network(&ws, &"x".repeat(50)));
    }

    #[test]
    fn short_network_names_unchanged() {
        assert_eq!(pair_network("ws", "svc"), "ws_net_svc");
    }
}
