mod harness;

use bollard_stubs::models::ContainerSummaryStateEnum;
use harness::{TestEnv, cleanup, unique_key, write_cfg};

// ── lifecycle ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_failed_clone_leaves_no_container_behind() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("lc-clone-fail");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    let before = env.git_containers().await;

    // the clone container mounts the ssh key volume: it must go whatever fails
    let out = env
        .rooz()
        .args([
            "new",
            &key,
            "--image",
            "alpine:latest",
            "--git-ssh-url",
            "https://h.invalid/none.git",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "a clone of a host that does not exist succeeded"
    );
    assert_eq!(
        env.git_containers().await,
        before,
        "the clone container outlived the failure: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    env.rooz().args(["rm", &key, "--force"]).output().ok();
}

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

// Without 'system init', 'new' must refuse before mounting the key: the mount alone would make the
// engine create an empty, unlabelled key volume that init then refuses to reuse.
#[tokio::test]
async fn new_without_ssh_key_refuses_and_creates_nothing() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("no-key");
    let ssh_vol = "rooz_ssh-key";

    env.rooz().args(["system", "prune"]).assert().success();
    if env.volume_exists(ssh_vol).await {
        env.remove_decoy_volume(ssh_vol).await;
    }

    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("rooz system init"));
    assert!(
        !env.volume_exists(ssh_vol).await,
        "a refused 'new' must not leave a key volume behind"
    );

    // a key volume left by an older rooz: refused, with the way out
    env.create_decoy_volume(ssh_vol).await;
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("remove it with"));
    env.remove_decoy_volume(ssh_vol).await;

    env.rooz().args(["system", "init"]).assert().success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();
    env.rooz().args(["rm", &key, "--force"]).assert().success();
}

// ── dry run ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dry_run_reports_and_changes_nothing() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("lc-dry");
    let cfg_path = harness::write_cfg(
        &key,
        "image: alpine:latest\nenv:\n  TOKEN: hush-MARKER\ninstall:\n  00-a:\n    run: echo plaintext-MARKER\n    root: true\nsidecars:\n  side:\n    image: alpine:latest\n",
    );

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();

    let out = env
        .rooz()
        .args(["new", &key, "--config", &cfg_path, "--dry-run"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    for expected in [
        "create   work",
        "create   sidecar   side",
        "create   network",
        "run      install   00-a  root",
        "Dry run: nothing was changed.",
    ] {
        assert!(
            stdout.contains(expected),
            "missing {:?} in:\n{}",
            expected,
            stdout
        );
    }
    assert!(
        !stdout.contains("MARKER"),
        "a plan must not print env values or scripts:\n{}",
        stdout
    );

    assert!(env.containers_by_workspace(&key).await.is_empty());
    assert!(env.volumes_by_workspace(&key).await.is_empty());
    assert!(env.network_names_by_workspace(&key).await.is_empty());

    // nothing was left behind to collide with the real run
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let out = env
        .rooz()
        .args(["update", &key, "--dry-run"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    assert!(stdout.contains("remove   container"), "{}", stdout);
    assert!(stdout.contains("reuse    volume"), "{}", stdout);
    assert!(!env.containers_by_workspace(&key).await.is_empty());

    harness::cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn dry_run_refuses_what_a_run_refuses() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("lc-dry-refuse");
    let cfg_path = harness::write_cfg(
        &key,
        "image: alpine:latest\nsidecars:\n  side:\n    image: alpine:latest\n    privileged: true\n",
    );

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();

    let out = env
        .rooz()
        .args(["new", &key, "--config", &cfg_path, "--dry-run"])
        .env_remove("ROOZ_ALLOW_PRIVILEGED")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).to_string();
    assert!(stderr.contains("privileged"), "{}", stderr);
    assert!(env.volumes_by_workspace(&key).await.is_empty());

    let _ = std::fs::remove_file(&cfg_path);
}

#[tokio::test]
async fn exec_runs_one_command_and_returns_its_exit_code() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("exec");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();

    let out = env
        .rooz()
        .args([
            "exec",
            &key,
            "--",
            "sh",
            "-c",
            "echo out; echo err >&2; exit 7",
        ])
        .assert()
        .code(7);
    let out = out.get_output();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "out\n");
    assert!(String::from_utf8_lossy(&out.stderr).contains("err"));

    env.rooz().args(["rm", &key, "--force"]).assert().success();
}

#[tokio::test]
async fn list_status_and_logs_describe_a_workspace() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("inspect");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();

    let list = env.rooz().args(["list", "--json"]).assert().success();
    let rows: serde_json::Value =
        serde_json::from_slice(&list.get_output().stdout).expect("list --json is JSON");
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == key.as_str())
        .expect("workspace listed");
    assert_eq!(row["status"], "running");
    assert!(row["image"].as_str().unwrap().contains("alpine"));

    let status = env
        .rooz()
        .args(["status", &key, "--json"])
        .assert()
        .success();
    let st: serde_json::Value = serde_json::from_slice(&status.get_output().stdout).unwrap();
    assert_eq!(st["name"], key.as_str());
    assert!(!st["containers"].as_array().unwrap().is_empty());
    assert!(!st["volumes"].as_array().unwrap().is_empty());

    env.rooz().args(["status", &key]).assert().success();
    env.rooz()
        .args(["logs", &key, "--tail", "5"])
        .assert()
        .success();
    env.rooz()
        .args(["status", "no-such-workspace-xyz"])
        .assert()
        .failure();

    env.rooz().args(["rm", &key, "--force"]).assert().success();
}

#[tokio::test]
async fn doctor_passes_after_init_and_fails_without_an_engine() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz().arg("doctor").assert().success();

    env.rooz()
        .env("DOCKER_HOST", "unix:///nonexistent-rooz.sock")
        .arg("doctor")
        .assert()
        .failure();
}

#[tokio::test]
async fn config_show_resolved_names_the_layer_of_each_setting() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("resolved");

    let dir = std::env::temp_dir().join(format!("rooz-test-{}", key));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("base.yaml"),
        "image: alpine:latest\nenv: {A: '1', B: '1'}\n",
    )
    .unwrap();
    let cfg_path = dir.join("ws.yaml");
    std::fs::write(&cfg_path, "bases: [base.yaml]\nenv: {B: '2'}\n").unwrap();
    let cfg_path = cfg_path.to_string_lossy().into_owned();

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let out = env
        .rooz()
        .args(["config", "show", &key, "--resolved"])
        .assert()
        .success();
    let out = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    let row = |setting: &str| {
        out.lines()
            .find(|l| l.split_whitespace().next() == Some(setting))
            .unwrap_or_else(|| panic!("no row for {}:\n{}", setting, out))
            .to_string()
    };
    assert!(row("image").contains("base.yaml"), "{}", out);
    assert!(row("env.A").contains("base.yaml"), "{}", out);
    assert!(row("env.B").contains("ws.yaml"), "{}", out);
    assert!(row("user").contains("default"), "{}", out);

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn update_that_cannot_resolve_leaves_the_workspace_untouched() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("up-gate");
    let cfg_path = write_cfg(&key, "image: alpine:latest\n");
    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();
    let before = env.volumes_by_workspace(&key).await.len();

    // an undefined variable fails while the configuration is resolved - before, that happened
    // only after the containers (and with --purge the volumes) were gone
    std::fs::write(&cfg_path, "image: \"alpine:{{ nope }}\"\n").unwrap();
    env.rooz()
        .args(["update", &key, "--purge"])
        .assert()
        .failure();

    assert!(!env.containers_by_workspace(&key).await.is_empty());
    assert_eq!(env.volumes_by_workspace(&key).await.len(), before);

    cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn update_keeps_the_uid_and_image_a_flags_only_workspace_was_made_with() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("up-uid");
    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest", "--uid", "2000"])
        .assert()
        .success();
    env.rooz()
        .args(["update", &key, "--no-pull"])
        .assert()
        .success();

    let out = env
        .rooz()
        .args(["exec", &key, "--", "id", "-u"])
        .assert()
        .success();
    assert_eq!(
        String::from_utf8_lossy(&out.get_output().stdout).trim(),
        "2000"
    );
    // flags are all such a workspace has: the image must not fall back to the default
    env.rooz()
        .args(["exec", &key, "--", "cat", "/etc/alpine-release"])
        .assert()
        .success();

    env.rooz().args(["rm", &key, "--force"]).assert().success();
}

#[tokio::test]
async fn a_config_that_does_not_parse_creates_nothing() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("bad-cfg");
    let cfg_path = write_cfg(&key, "image: alpine:latest\nimge: typo\n");
    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();

    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .failure();

    assert!(env.containers_by_workspace(&key).await.is_empty());
    assert!(env.volumes_by_workspace(&key).await.is_empty());
    cleanup(&env, &key, &cfg_path);
}
