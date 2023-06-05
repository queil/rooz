use std::collections::HashMap;

pub const WORKSPACE_KEY: &'static str = "dev.rooz.workspace";
const ROLE: &'static str = "dev.rooz.role";
const ROOZ: &'static str = "dev.rooz";
const LABEL_KEY: &'static str = "label";
const TRUE: &'static str = "true";

struct Label {
    key: String,
    value: String,
    formatted: String,
}

impl Label {
    fn new(key: &str, value: &str) -> Label {
        Label {
            key: key.into(),
            value: value.into(),
            formatted: format!("{}={}", key, value),
        }
    }
}

pub struct Labels {
    rooz: Label,
    workspace: Option<Label>,
    role: Option<Label>,
}

impl Labels {
    pub fn new(workspace_key: Option<&str>, role: Option<&str>) -> Labels {
        Labels {
            rooz: Label::new(ROOZ, TRUE),
            workspace: workspace_key.map(|v| Label::new(WORKSPACE_KEY, v)),
            role: role.map(|v| Label::new(ROLE, v)),
        }
    }
}

impl<'a> From<&'a Labels> for HashMap<&'a str, &'a str> {
    fn from(value: &'a Labels) -> Self {
        let labels:Vec::<&Label> = value.into();
        let mut h = HashMap::new();
        for l in labels {
            h.insert(l.key.as_ref(), l.value.as_ref());
        }
        return h;
    }
}

impl<'a> From<&'a Labels> for HashMap<String, Vec<String>> {
    fn from(value: &'a Labels) -> Self {
        let labels:Vec::<&Label> = value.into();
        let mut h = HashMap::new();
        for l in labels {
            h.insert(LABEL_KEY.into(), vec![l.formatted.to_string()]);
        }
        return h;
    }
}

impl<'a> From<&'a Labels> for Vec<&'a Label> {
    fn from(value: &'a Labels) -> Self {
        let mut labels = vec![&value.rooz];
        if let Some(role) = &value.role {
            labels.push(role);
        }
        if let Some(workspace) = &value.workspace {
            labels.push(workspace);
        }
        labels
    }
}
