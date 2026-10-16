use std::collections::HashMap;

use serde_yaml::Value;

use super::config::RoozCfg;
use crate::model::types::AnyError;

#[derive(Debug, PartialEq)]
pub struct ResolvedRow {
    pub path: String,
    pub value: String,
    pub from: String,
}

struct Leaf {
    path: String,
    // what identifies the leaf across layers: the path, plus the item for list entries, whose
    // index shifts as layers append to (or sort into) the list
    key: String,
    value: String,
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn flatten(v: &Value, path: &str, out: &mut Vec<Leaf>) {
    match v {
        Value::Mapping(m) => {
            for (k, v) in m {
                let k = scalar(k).unwrap_or_default();
                let p = if path.is_empty() {
                    k
                } else {
                    format!("{}.{}", path, k)
                };
                flatten(v, &p, out);
            }
        }
        Value::Sequence(items) => {
            for (i, item) in items.iter().enumerate() {
                match scalar(item) {
                    Some(s) => out.push(Leaf {
                        path: format!("{}[{}]", path, i),
                        key: format!("{}[]={}", path, s),
                        value: s,
                    }),
                    None => flatten(item, &format!("{}[{}]", path, i), out),
                }
            }
        }
        Value::Tagged(t) => flatten(&t.value, path, out),
        other => {
            if let Some(s) = scalar(other) {
                out.push(Leaf {
                    path: path.to_string(),
                    key: path.to_string(),
                    value: s,
                });
            }
        }
    }
}

fn leaves(cfg: &RoozCfg) -> Result<Vec<Leaf>, AnyError> {
    let mut out = Vec::new();
    flatten(&serde_yaml::to_value(cfg)?, "", &mut out);
    Ok(out)
}

/// The merged configuration, one row per setting, each with the layer where its final value
/// took effect. Layers merge in order, exactly as `RoozCfg::from_config` merges them, so the
/// merge rules are not restated here: a setting belongs to the earliest layer from which the
/// merged result already holds its final value.
pub fn resolve(layers: &[(String, RoozCfg)]) -> Result<Vec<ResolvedRow>, AnyError> {
    let mut acc = RoozCfg::none();
    let mut prefixes: Vec<HashMap<String, String>> = Vec::new();
    let mut last = Vec::new();
    for (_, layer) in layers {
        acc.from_config(layer);
        let l = leaves(&acc)?;
        prefixes.push(l.iter().map(|x| (x.key.clone(), x.value.clone())).collect());
        last = l;
    }

    Ok(last
        .into_iter()
        .map(|leaf| {
            let mut from = layers.len() - 1;
            while from > 0
                && prefixes[from - 1]
                    .get(&leaf.key)
                    .is_some_and(|v| *v == leaf.value)
            {
                from -= 1;
            }
            ResolvedRow {
                path: leaf.path,
                value: leaf.value,
                from: layers[from].0.clone(),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config::FileFormat;

    fn cfg(yaml: &str) -> RoozCfg {
        RoozCfg::from_string(yaml, FileFormat::Yaml).unwrap()
    }

    fn from_of(rows: &[ResolvedRow], path: &str) -> String {
        rows.iter()
            .find(|r| r.path == path)
            .unwrap_or_else(|| panic!("no row {}", path))
            .from
            .clone()
    }

    #[test]
    fn each_setting_names_the_layer_it_took_effect_in() {
        let rows = resolve(&[
            ("default".into(), RoozCfg::default()),
            (
                "base.yaml".into(),
                cfg("image: alpine\nenv: {A: '1', B: '1'}\ncaches: ['~/.cargo']\n"),
            ),
            (
                "main.yaml".into(),
                cfg("env: {B: '2'}\ncaches: ['~/.nuget']\nuser: dev\n"),
            ),
        ])
        .unwrap();

        assert_eq!(from_of(&rows, "image"), "base.yaml");
        assert_eq!(from_of(&rows, "env.A"), "base.yaml");
        assert_eq!(from_of(&rows, "env.B"), "main.yaml");
        assert_eq!(from_of(&rows, "caches[0]"), "base.yaml");
        assert_eq!(from_of(&rows, "caches[1]"), "main.yaml");
        assert_eq!(from_of(&rows, "user"), "main.yaml");
        assert_eq!(from_of(&rows, "shell[0]"), "default");
    }

    #[test]
    fn a_value_repeated_by_a_later_layer_stays_with_the_first() {
        let rows = resolve(&[
            ("base.yaml".into(), cfg("image: alpine\n")),
            ("main.yaml".into(), cfg("image: alpine\n")),
        ])
        .unwrap();
        assert_eq!(from_of(&rows, "image"), "base.yaml");
    }

    #[test]
    fn sidecar_settings_merge_per_field() {
        let rows = resolve(&[
            (
                "base.yaml".into(),
                cfg("sidecars: {db: {image: pg, env: {X: '1'}}}\n"),
            ),
            ("main.yaml".into(), cfg("sidecars: {db: {image: pg16}}\n")),
        ])
        .unwrap();
        assert_eq!(from_of(&rows, "sidecars.db.image"), "main.yaml");
        assert_eq!(from_of(&rows, "sidecars.db.env.X"), "base.yaml");
    }
}
