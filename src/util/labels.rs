use std::{collections::HashMap, vec};

pub const WORKSPACE_KEY: &'static str = "dev.rooz.workspace";
pub const CONTAINER: &'static str = "dev.rooz.workspace.container";
pub const ROLE: &'static str = "dev.rooz.role";
pub const RUNTIME_CONFIG: &'static str = "dev.rooz.config.runtime";
pub const CONFIG_ORIGIN: &'static str = "dev.rooz.config.origin";
pub const CONFIG_BODY: &'static str = "dev.rooz.config.body";
const ROOZ: &'static str = "dev.rooz";
pub const LABEL_KEY: &'static str = "label";
const TRUE: &'static str = "true";

//pub const HOME_ROLE: &'static str = "home";
pub const WORK_ROLE: &'static str = "work";
pub const DATA_ROLE: &'static str = "data";
pub const SSH_KEY_ROLE: &'static str = "ssh-key";
pub const WORKSPACE_CONFIG_ROLE: &'static str = "workspace-config";
pub const SYSTEM_CONFIG_ROLE: &'static str = "sys-config";
pub const CACHE_ROLE: &'static str = "cache";
pub const SIDECAR_ROLE: &'static str = "sidecar";

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
        return h;
    }

    pub fn to_vec_str<'a>(key_values: &'a Vec<Self>) -> Vec<&'a str> {
        let mut v = vec![];
        for kv in key_values {
            v.push(kv.formatted.as_ref());
        }
        return v;
    }
}

#[derive(Clone, Debug)]
pub struct Labels(HashMap<String, String>);

impl Labels {
    pub fn new(map: HashMap<String, String>) -> Self {
        Self(map)
    }

    pub fn from(items: &[(&str, &str)]) -> Self {
        let mut map = HashMap::from(
            items
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<String, String>>(),
        );
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
                    (k, v) if v == "" => k.to_string(),
                    (k, v) => format!("{}={}", k, v),
                })
                .collect(),
        );
        return h;
    }
}
