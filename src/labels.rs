pub const WORKSPACE_KEY: &'static str = "dev.rooz.workspace";
pub const ROLE: &'static str = "dev.rooz.role";
pub const ROOZ: &'static str = "dev.rooz";

pub fn filter(key: &str, value: &str) -> String {
    format!("{}={}", key, value)
}

pub fn belongs_to(workspace_key: &str) -> String {
    filter(WORKSPACE_KEY, workspace_key)
}
