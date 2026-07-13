mod harness;

use harness::{TestEnv, unique_key};
use std::{fs, io::Write};

fn sidecar_cfg(key: &str) -> (String, String) {
    let path = format!("/tmp/rooz-test-sidecar-{}.yaml", key);
    let yaml = "\
image: alpine:latest
sidecars:
  svc:
    image: alpine:latest
    command:
      - sleep
    args:
      - infinity
";
    let mut f = fs::File::create(&path).expect("write sidecar config");
    f.write_all(yaml.as_bytes()).unwrap();
    (path, "svc".to_string())
}

// ── creation and labelling ────────────────────────────────────────────────────

#[tokio::test]
async fn sidecar_created_alongside_work_container() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-create");
    let (cfg_path, sidecar_name) = sidecar_cfg(&key);

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let containers = env.containers_by_workspace(&key).await;
    assert_eq!(
        containers.len(),
        2,
        "expected work container + 1 sidecar, got {} containers",
        containers.len()
    );

    let sidecar: Vec<_> = containers
        .iter()
        .filter(|c| {
            c.labels
                .as_ref()
                .and_then(|l| l.get("dev.rooz.role"))
                .map(String::as_str)
                == Some("sidecar")
        })
        .collect();

    assert_eq!(sidecar.len(), 1, "expected exactly one sidecar container");

    let sc_labels = sidecar[0].labels.as_ref().expect("sidecar has no labels");
    assert_eq!(
        sc_labels.get("dev.rooz.workspace").map(String::as_str),
        Some(key.as_str()),
        "sidecar missing workspace label"
    );
    assert_eq!(
        sc_labels
            .get("dev.rooz.workspace.container")
            .map(String::as_str),
        Some(sidecar_name.as_str()),
        "sidecar missing container-name label"
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    let _ = fs::remove_file(&cfg_path);
}

// ── removal ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sidecar_removed_with_workspace() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-rm");
    let (cfg_path, _) = sidecar_cfg(&key);

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    assert_eq!(
        env.containers_by_workspace(&key).await.len(),
        2,
        "setup: expected 2 containers"
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();

    let remaining = env.containers_by_workspace(&key).await;
    assert!(
        remaining.is_empty(),
        "rooz rm left {} containers behind (including sidecar)",
        remaining.len()
    );

    let _ = fs::remove_file(&cfg_path);
}

// ── stop / start ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn sidecar_stops_and_starts_with_workspace() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-stop");
    let (cfg_path, _) = sidecar_cfg(&key);

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let states = env.workspace_container_states(&key).await;
    assert_eq!(states.len(), 2, "expected 2 containers after new");

    env.rooz().args(["stop", &key]).assert().success();

    let stopped = env.workspace_container_states(&key).await;
    assert!(
        stopped
            .iter()
            .all(|s| *s == bollard_stubs::models::ContainerSummaryStateEnum::EXITED),
        "expected all containers exited after stop, got {:?}",
        stopped
    );

    env.rooz().args(["start", &key]).assert().success();

    let started = env.workspace_container_states(&key).await;
    assert!(
        started
            .iter()
            .all(|s| *s == bollard_stubs::models::ContainerSummaryStateEnum::RUNNING),
        "expected all containers running after start, got {:?}",
        started
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    let _ = fs::remove_file(&cfg_path);
}
