use rooz::config::config::{FileFormat, RoozCfg};

fn yaml_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            yaml_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "yaml") {
            out.push(path);
        }
    }
}

// The schema is generated from the structs that parse the config, so every example that
// parses must also validate - and a typo must not.
#[test]
fn schema_accepts_every_example_config_and_rejects_unknown_fields() {
    let schema: serde_json::Value = serde_json::from_str(&RoozCfg::json_schema().unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();

    let mut files = Vec::new();
    yaml_files(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples"),
        &mut files,
    );

    let mut checked = 0;
    for file in files {
        let body = std::fs::read_to_string(&file).unwrap();
        if RoozCfg::from_string(&body, FileFormat::Yaml).is_err() {
            continue;
        }
        let doc: serde_json::Value = serde_yaml::from_str(&body).unwrap();
        if let Err(e) = validator.validate(&doc) {
            panic!("{} does not validate: {}", file.display(), e);
        }
        checked += 1;
    }
    assert!(checked > 0, "no example configs were checked");

    let typo: serde_json::Value = serde_json::json!({ "cashes": ["~/.cargo"] });
    assert!(!validator.is_valid(&typo), "unknown field accepted");
}
