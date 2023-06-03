use std::collections::HashMap;

use crate::labels;

const LABEL_KEY: &'static str = "label";

pub fn all() -> HashMap<String, Vec<String>> {
    return HashMap::from([(LABEL_KEY.into(), vec![labels::ROOZ.to_string()])]);
}

pub fn of_workspace(key: &str) -> HashMap<String, Vec<String>> {
    let mut hs = all();
    hs.insert(LABEL_KEY.into(), vec![labels::belongs_to(&key.to_string())]);
    return hs;
}
