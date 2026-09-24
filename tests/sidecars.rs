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

fn install_sidecar_cfg(key: &str) -> (String, String) {
    let path = format!("/tmp/rooz-test-sidecar-install-{}.yaml", key);
    let yaml = "\
image: alpine:latest
sidecars:
  svc:
    image: alpine:latest
    install:
      01-mark: echo built-by-rooz > /rooz-install-marker
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

// ── runtime image provenance ──────────────────────────────────────────────────

// A sidecar with install steps is committed to localhost/rooz/<workspace>/<sidecar>:latest -
// a name anyone with engine access can take, with any labels they like on the image behind
// it. Rooz must run the sidecar from the image id it recorded itself, never from the name.
#[tokio::test]
async fn a_squatted_runtime_image_is_never_run() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-squat");
    let (cfg_path, sidecar_name) = install_sidecar_cfg(&key);
    let runtime_image = format!("localhost/rooz/{}/{}", key, sidecar_name);

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();

    // the co-tenant gets there first, with rooz's own labels forged onto the image
    let squatted = env
        .squat_image(
            &runtime_image,
            &[
                ("dev.rooz", "true"),
                ("dev.rooz.workspace", &key),
                ("dev.rooz.role", "sidecar-runtime"),
                ("dev.rooz.workspace.container", &sidecar_name),
            ],
            "/squat-marker",
        )
        .await;

    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let containers = env.containers_by_workspace(&key).await;
    let sidecar = containers
        .iter()
        .find(|c| {
            c.labels
                .as_ref()
                .and_then(|l| l.get("dev.rooz.role"))
                .map(String::as_str)
                == Some("sidecar")
        })
        .expect("sidecar container not found");
    let id = sidecar.id.as_deref().expect("sidecar has no id");
    let image_id = sidecar.image_id.as_deref().unwrap_or_default();
    let bare = |v: &str| v.trim_start_matches("sha256:").to_string();

    assert_ne!(
        bare(image_id),
        bare(&squatted),
        "the sidecar was created from the squatted image"
    );
    assert_eq!(
        env.exec_code(id, vec!["test", "-f", "/rooz-install-marker"])
            .await,
        0,
        "the configured install step did not run"
    );
    assert_eq!(
        env.exec_code(id, vec!["test", "-f", "/squat-marker"]).await,
        1,
        "the squatted image's content is inside the sidecar"
    );

    // what rooz built is recorded on the container, by id - that is what the next run reuses
    let pinned = sidecar
        .labels
        .as_ref()
        .and_then(|l| l.get("dev.rooz.runtime-image"))
        .cloned()
        .expect("no runtime image recorded on the sidecar");
    assert_eq!(
        bare(&pinned),
        bare(image_id),
        "the recorded image id is not the image the sidecar runs"
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    env.remove_image(&squatted).await;
    let _ = fs::remove_file(&cfg_path);
}
