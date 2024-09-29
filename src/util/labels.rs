use std::{collections::HashMap, vec};

use crate::config::runtime::RuntimeConfig;

pub const WORKSPACE_KEY: &'static str = "dev.rooz.workspace";
pub const CONTAINER: &'static str = "dev.rooz.workspace.container";
pub const ROLE: &'static str = "dev.rooz.role";
pub const RUNTIME_CONFIG: &'static str = "dev.rooz.config.runtime";
pub const CONFIG_ORIGIN: &'static str = "dev.rooz.config.origin";
pub const CONFIG_BODY: &'static str = "dev.rooz.config.body";
const ROOZ: &'static str = "dev.rooz";
const LABEL_KEY: &'static str = "label";
const TRUE: &'static str = "true";

pub const ROLE_WORK: &'static str = "work";
pub const ROLE_SIDECAR: &'static str = "sidecar";

#[derive(Clone, Debug)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
    formatted: String,
}

impl KeyValue {
    pub fn new(key: &str, value: &str) -> Self {
        KeyValue {
            key: key.into(),
            value: value.into(),
            formatted: format!("{}={}", key, value),
        }
    }

    pub fn to_hashmap_of_ref<'a>(value: &'a HashMap<String, String>) -> HashMap<&'a str, &'a str> {
        let mut h = HashMap::new();
        for (key, value) in value {
            h.insert(key.as_ref(), value.as_ref());
        }
        return h;
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
pub struct Labels {
    pub rooz: KeyValue,
    pub workspace: Option<KeyValue>,
    pub container: Option<KeyValue>,
    pub runtime_config: Option<KeyValue>,
    pub role: Option<KeyValue>,
    pub config_source: Option<KeyValue>,
    pub config_body: Option<KeyValue>,
}

impl Labels {
    pub fn new(workspace_key: Option<&str>, role: Option<&str>) -> Labels {
        Labels {
            workspace: workspace_key.map(|v| KeyValue::new(WORKSPACE_KEY, v)),
            role: role.map(|v| KeyValue::new(ROLE, v)),
            ..Default::default()
        }
    }

    pub fn workspace(key: &str) -> Option<KeyValue> {
        Some(KeyValue::new(WORKSPACE_KEY, key))
    }

    pub fn role(role: &str) -> Option<KeyValue> {
        Some(KeyValue::new(ROLE, role))
    }

    pub fn config_origin(path: &str) -> Option<KeyValue> {
        Some(KeyValue::new(CONFIG_ORIGIN, path))
    }

    pub fn config_body(body: &str) -> Option<KeyValue> {
        Some(KeyValue::new(CONFIG_BODY, body))
    }

    pub fn with_role(self, role: &str) -> Labels {
        Labels {
            role: Some(KeyValue::new(ROLE, role)),
            ..self
        }
    }

    pub fn with_container(self, container: Option<&str>) -> Labels {
        match container {
            Some(c) => Labels {
                container: Some(KeyValue::new(CONTAINER, c)),
                ..self
            },
            None => self,
        }
    }

    pub fn with_runtime_config(self, config: RuntimeConfig) -> Self {
        Labels {
            runtime_config: Some(KeyValue::new(RUNTIME_CONFIG, &config.to_string().unwrap())),
            ..self
        }
    }
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            rooz: KeyValue::new(ROOZ, TRUE),
            workspace: None,
            container: None,
            runtime_config: None,
            role: None,
            config_source: None,
            config_body: None,
        }
    }
}

impl<'a> From<&'a Labels> for HashMap<&'a str, &'a str> {
    fn from(value: &'a Labels) -> Self {
        let labels: Vec<&KeyValue> = value.into();
        let mut h = HashMap::new();
        for l in labels {
            h.insert(l.key.as_ref(), l.value.as_ref());
        }
        return h;
    }
}

impl<'a> From<&'a Labels> for HashMap<String, Vec<String>> {
    fn from(value: &'a Labels) -> Self {
        let labels: Vec<&KeyValue> = value.into();
        let mut h = HashMap::new();
        h.insert(
            LABEL_KEY.into(),
            labels.iter().map(|v| v.formatted.to_string()).collect(),
        );
        return h;
    }
}

impl<'a> From<&'a Labels> for Vec<&'a KeyValue> {
    fn from(value: &'a Labels) -> Self {
        let mut labels = vec![&value.rooz];
        if let Some(role) = &value.role {
            labels.push(role);
        }
        if let Some(value) = &value.workspace {
            labels.push(value);
        }
        if let Some(value) = &value.container {
            labels.push(value);
        }
        if let Some(value) = &value.runtime_config {
            labels.push(value);
        }
        if let Some(value) = &value.config_source {
            labels.push(value);
        }
        if let Some(value) = &value.config_body {
            labels.push(value);
        }
        labels
    }
}
