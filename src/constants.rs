pub const DEFAULT_IMAGE: &'static str = "docker.io/bitnami/git:latest";
pub const DEFAULT_SHELL: &'static str = "sh";
pub const DEFAULT_USER: &'static str = "rooz_user";
pub const DEFAULT_UID: u32 = 1000;
pub const DEFAULT_CONTAINER_NAME: &'static str = "work";
pub const ROOT_UID: &'static str = "0";
pub const ROOT_UID_INT: u32 = 0;
pub const ROOT_USER: &'static str = "root";
pub const WORK_DIR: &'static str = "/work";
pub fn default_entrypoint<'a>() -> Option<Vec<&'a str>> {
    Some(vec!["cat"])
}
