use std::{collections::HashMap, vec};

pub const WORKSPACE_KEY: &str = "dev.rooz.workspace";
pub const CONTAINER: &str = "dev.rooz.workspace.container";
pub const ROLE: &str = "dev.rooz.role";
pub const RUNTIME_CONFIG: &str = "dev.rooz.config.runtime";
pub const CONFIG_ORIGIN: &str = "dev.rooz.config.origin";
pub const CONFIG_BODY: &str = "dev.rooz.config.body";
pub const FORWARD_PORTS: &str = "dev.rooz.forward-ports";
pub const RUNTIME_IMAGE: &str = "dev.rooz.runtime-image";
pub const BUILD_KEY: &str = "dev.rooz.build-key";
pub const ROOZ: &str = "dev.rooz";
pub const LABEL_KEY: &str = "label";
const TRUE: &str = "true";

pub const WORK_ROLE: &str = "work";
pub const DATA_ROLE: &str = "data";
pub const SSH_KEY_ROLE: &str = "ssh-key";
pub const WORKSPACE_CONFIG_ROLE: &str = "workspace-config";
pub const SYSTEM_CONFIG_ROLE: &str = "sys-config";
pub const CACHE_ROLE: &str = "cache";
pub const SIDECAR_ROLE: &str = "sidecar";
pub const SIDECAR_RUNTIME_ROLE: &str = "sidecar-runtime";
pub const INSTALL_CHECKPOINT_ROLE: &str = "install-checkpoint";

#[derive(Clone, Debug)]
pub struct KeyValue {
    formatted: String,
}

impl KeyValue {
    pub fn new(key: &str, value: &str) -> Self {
        KeyValue {
            formatted: format!("{}={}", key, value),
        }
    }

    pub fn to_vec(value: HashMap<String, String>) -> Vec<Self> {
        let mut h = Vec::new();
        for (key, value) in value {
            h.push(Self::new(&key, &value));
        }
        h
    }

    pub fn to_vec_str(key_values: &Vec<Self>) -> Vec<&str> {
        let mut v = vec![];
        for kv in key_values {
            v.push(kv.formatted.as_ref());
        }
        v
    }
}

#[derive(Clone, Debug)]
pub struct Labels(HashMap<String, String>);

impl Labels {
    pub fn new(map: HashMap<String, String>) -> Self {
        Self(map)
    }

    pub fn from(items: &[(&str, &str)]) -> Self {
        let mut map = items
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<String, String>>();
        map.insert(ROOZ.to_string(), TRUE.to_string());
        Labels::new(map)
    }

    pub fn append(&mut self, item: (&str, &str)) {
        self.0.insert(item.0.to_string(), item.1.to_string());
    }

    pub fn extend(&mut self, items: &[(&str, &str)]) {
        for item in items {
            self.append(*item);
        }
    }

    pub fn extend_with_labels(&mut self, items: Labels) {
        for (k, v) in items.0 {
            self.append((&k, &v));
        }
    }

    pub fn workspace(key: &str) -> (&str, &str) {
        (WORKSPACE_KEY, key)
    }

    pub fn any_workspace() -> (&'static str, &'static str) {
        (WORKSPACE_KEY, "")
    }

    pub fn container(key: &str) -> (&str, &str) {
        (CONTAINER, key)
    }

    pub fn config_runtime(value: &str) -> (&str, &str) {
        (RUNTIME_CONFIG, value)
    }

    pub fn config_origin(path: &str) -> (&str, &str) {
        (CONFIG_ORIGIN, path)
    }

    pub fn config_body(body: &str) -> (&str, &str) {
        (CONFIG_BODY, body)
    }

    pub fn role(role: &str) -> (&str, &str) {
        (ROLE, role)
    }

    pub fn workspace_config(key: &str) -> Labels {
        Labels::from(&[Labels::workspace(key), Labels::role(WORKSPACE_CONFIG_ROLE)])
    }

    pub fn runtime_image(image_id: &str) -> (&str, &str) {
        (RUNTIME_IMAGE, image_id)
    }

    pub fn build_key(key: &str) -> (&str, &str) {
        (BUILD_KEY, key)
    }

    // Every label expected has to be present with the same value; extra labels on the object
    // are fine (engines add their own, and rooz stamps more of them over time).
    pub fn is_subset_of(&self, found: &HashMap<String, String>) -> bool {
        self.0
            .iter()
            .all(|(k, v)| found.get(k).map(|f| f == v).unwrap_or(false))
    }
}

impl From<Labels> for HashMap<String, String> {
    fn from(value: Labels) -> Self {
        value.0
    }
}

impl Default for Labels {
    fn default() -> Self {
        Self::from(&[])
    }
}

impl From<Labels> for HashMap<String, Vec<String>> {
    fn from(value: Labels) -> Self {
        let mut h = HashMap::new();
        h.insert(
            LABEL_KEY.into(),
            value
                .0
                .iter()
                .map(|x| match x {
                    (k, v) if v.is_empty() => k.to_string(),
                    (k, v) => format!("{}={}", k, v),
                })
                .collect(),
        );
        h
    }
}
