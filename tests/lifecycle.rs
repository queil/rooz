mod harness;

use bollard_stubs::models::ContainerSummaryStateEnum;
use harness::{TestEnv, unique_key};

// ── lifecycle ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn stop_and_start() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("lc-stop");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();

    let states = env.workspace_container_states(&key).await;
    assert!(!states.is_empty(), "expected containers after rooz new");
    assert!(
        states
            .iter()
            .all(|s| *s == ContainerSummaryStateEnum::RUNNING),
        "expected all containers running after rooz new, got {:?}",
        states
    );

    env.rooz().args(["stop", &key]).assert().success();

    let states = env.workspace_container_states(&key).await;
    assert!(
        states
            .iter()
            .all(|s| *s == ContainerSummaryStateEnum::EXITED),
        "expected all containers exited after rooz stop, got {:?}",
        states
    );

    env.rooz().args(["start", &key]).assert().success();

    let states = env.workspace_container_states(&key).await;
    assert!(
        states
            .iter()
            .all(|s| *s == ContainerSummaryStateEnum::RUNNING),
        "expected all containers running after rooz start, got {:?}",
        states
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();
}

#[tokio::test]
async fn restart() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("lc-restart");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();

    env.rooz().args(["restart", &key]).assert().success();

    let states = env.workspace_container_states(&key).await;
    assert!(
        states
            .iter()
            .all(|s| *s == ContainerSummaryStateEnum::RUNNING),
        "expected all containers running after rooz restart, got {:?}",
        states
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();
}

#[tokio::test]
async fn list_shows_workspace() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("lc-list");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();

    env.rooz().args(["list"]).assert().success();

    env.rooz().args(["rm", &key, "--force"]).assert().success();
}

// ── destructive-safety ───────────────────────────────────────────────────────

#[tokio::test]
async fn rm_ignores_decoy_container() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("ds-rm");
    let decoy = unique_key("decoy-container");

    env.create_decoy_container(&decoy).await;

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();

    env.rooz().args(["rm", &key, "--force"]).assert().success();

    assert!(
        env.container_exists(&decoy).await,
        "rooz rm removed a non-rooz container — destructive-safety failure"
    );

    env.remove_decoy_container(&decoy).await;
}

#[tokio::test]
async fn prune_ignores_decoy_container() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let decoy = unique_key("decoy-prune-c");

    env.create_decoy_container(&decoy).await;

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz().args(["system", "prune"]).assert().success();

    assert!(
        env.container_exists(&decoy).await,
        "rooz system prune removed a non-rooz container — destructive-safety failure"
    );

    env.remove_decoy_container(&decoy).await;
}

#[tokio::test]
async fn prune_ignores_decoy_volume() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let decoy = unique_key("decoy-prune-v");

    env.create_decoy_volume(&decoy).await;

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz().args(["system", "prune"]).assert().success();

    assert!(
        env.volume_exists(&decoy).await,
        "rooz system prune removed a non-rooz volume — destructive-safety failure"
    );

    env.remove_decoy_volume(&decoy).await;
}

#[tokio::test]
async fn rm_ignores_decoy_volume() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("ds-rmvol");
    let decoy = unique_key("decoy-volume");

    env.create_decoy_volume(&decoy).await;

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();

    env.rooz().args(["rm", &key, "--force"]).assert().success();

    assert!(
        env.volume_exists(&decoy).await,
        "rooz rm removed a non-rooz volume — destructive-safety failure"
    );

    env.remove_decoy_volume(&decoy).await;
}
