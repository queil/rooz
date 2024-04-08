use std::{collections::HashMap, vec};

use crate::model::config::FinalCfg;

pub const WORKSPACE_KEY: &'static str = "dev.rooz.workspace";
pub const CONTAINER: &'static str = "dev.rooz.workspace.container";
pub const ROLE: &'static str = "dev.rooz.role";
pub const CONFIG: &'static str = "dev.rooz.config";
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
    pub fn new(key: &str, value: &str) -> KeyValue {
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

    pub fn to_vec(value: HashMap<String, String>) -> Vec<KeyValue> {
        let mut h = Vec::new();
        for (key, value) in value {
            h.push(Self::new(&key, &value));
        }
        return h;
    }

    pub fn to_vec_str<'a>(key_values: &'a Vec<KeyValue>) -> Vec<&'a str> {
        let mut v = vec![];
        for kv in key_values {
            v.push(kv.formatted.as_ref());
        }
        return v;
    }
}

#[derive(Clone, Debug)]
pub struct Labels {
    rooz: KeyValue,
    workspace: Option<KeyValue>,
    container: Option<KeyValue>,
    config: Option<KeyValue>,
    role: Option<KeyValue>,
}

impl Labels {
    pub fn new(workspace_key: Option<&str>, role: Option<&str>) -> Labels {
        Labels {
            rooz: KeyValue::new(ROOZ, TRUE),
            workspace: workspace_key.map(|v| KeyValue::new(WORKSPACE_KEY, v)),
            container: None,
            config: None,
            role: role.map(|v| KeyValue::new(ROLE, v)),
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

    pub fn with_config(self, config: FinalCfg) -> Self {
        Labels {
            config: Some(KeyValue::new(CONFIG, &config.to_string().unwrap())),
            ..self
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
        if let Some(workspace) = &value.workspace {
            labels.push(workspace);
        }
        if let Some(container) = &value.container {
            labels.push(container);
        }
        if let Some(config) = &value.config {
            labels.push(config);
        }
        labels
    }
}
