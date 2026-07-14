pub const DEFAULT_IMAGE: &'static str = "docker.io/chainguard/git:latest-dev";
pub const DEFAULT_SHELL: &'static str = "sh";
pub const DEFAULT_USER: &'static str = "rooz_user";
pub const DEFAULT_UID: &'static str = "1000";
pub const DEFAULT_CONTAINER_NAME: &'static str = "work";
pub const ROOT_UID: &'static str = "0";
pub const ROOT_UID_INT: i32 = 0;
pub const ROOT_USER: &'static str = "root";
pub const WORK_DIR: &'static str = "/work";
pub fn default_command<'a>() -> Option<Vec<&'a str>> {
    Some(vec!["cat"])
}

pub fn egress_network(workspace_key: &str) -> String {
    format!("{}-egress", workspace_key)
}
pub fn pair_network(workspace_key: &str, sidecar: &str) -> String {
    fit_network_name(format!("{}-net-{}", workspace_key, sidecar))
}
pub fn peer_network(workspace_key: &str, a: &str, b: &str) -> String {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    fit_network_name(format!("{}-peer-{}-{}", workspace_key, a, b))
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
        assert_eq!(peer_network("ws", "a", "b"), "ws-peer-a-b");
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
        assert_eq!(pair_network("ws", "svc"), "ws-net-svc");
    }
}
