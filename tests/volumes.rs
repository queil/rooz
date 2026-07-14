mod harness;

use harness::{TestEnv, unique_key};
use std::{fs, io::Write};

// Cache volume name is deterministic from the path. sanitize("~/.cargo") == "---cargo".
const CARGO_CACHE_VOL: &str = "rooz_cache_---cargo";

// ── label correctness ────────────────────────────────────────────────────────

#[tokio::test]
async fn workspace_volumes_carry_workspace_label() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("vol-lbl");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();

    let vols = env.volumes_by_workspace(&key).await;
    assert!(!vols.is_empty(), "expected at least one workspace volume");
    for v in &vols {
        let labels = &v.labels;
        assert_eq!(
            labels.get("dev.rooz.workspace").map(String::as_str),
            Some(key.as_str()),
            "volume {} missing dev.rooz.workspace label",
            v.name
        );
        assert_eq!(
            labels.get("dev.rooz").map(String::as_str),
            Some("true"),
            "volume {} missing dev.rooz=true label",
            v.name
        );
    }

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    assert!(
        env.volumes_by_workspace(&key).await.is_empty(),
        "workspace volumes remain after rm"
    );
}

// ── cache volumes ────────────────────────────────────────────────────────────

#[tokio::test]
async fn cache_volume_survives_workspace_rm() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("vol-cache");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args([
            "new",
            &key,
            "--image",
            "alpine:latest",
            "--caches",
            "~/.cargo",
        ])
        .assert()
        .success();

    assert!(
        env.volume_exists(CARGO_CACHE_VOL).await,
        "cache volume not created"
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();

    assert!(
        env.volumes_by_workspace(&key).await.is_empty(),
        "workspace volumes remain after rm"
    );
    assert!(
        env.volume_exists(CARGO_CACHE_VOL).await,
        "cache volume was removed by rooz rm — it should persist"
    );

    env.remove_decoy_volume(CARGO_CACHE_VOL).await;
}

#[tokio::test]
async fn cache_volume_shared_across_workspaces() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key1 = unique_key("vol-sh1");
    let key2 = unique_key("vol-sh2");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args([
            "new",
            &key1,
            "--image",
            "alpine:latest",
            "--caches",
            "~/.cargo",
        ])
        .assert()
        .success();
    env.rooz()
        .args([
            "new",
            &key2,
            "--image",
            "alpine:latest",
            "--caches",
            "~/.cargo",
        ])
        .assert()
        .success();

    // One cache volume, not two
    let all_rooz = env.all_rooz_volumes().await;
    let cache_vols: Vec<_> = all_rooz
        .iter()
        .filter(|v| v.name == CARGO_CACHE_VOL)
        .collect();
    assert_eq!(
        cache_vols.len(),
        1,
        "expected exactly one shared cache volume, got {}",
        cache_vols.len()
    );

    // Cache volume has no workspace label — it belongs to all workspaces
    let cache_labels = &cache_vols[0].labels;
    assert!(
        !cache_labels.contains_key("dev.rooz.workspace"),
        "cache volume must not carry a workspace label"
    );

    env.rooz().args(["rm", &key1, "--force"]).assert().success();
    env.rooz().args(["rm", &key2, "--force"]).assert().success();

    assert!(
        env.volume_exists(CARGO_CACHE_VOL).await,
        "cache volume was removed when both workspaces were deleted — it should persist"
    );

    env.remove_decoy_volume(CARGO_CACHE_VOL).await;
}

// ── inline data ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn inline_data_content_written_to_volume() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("vol-inline");

    // Volume name: rooz-{key}-greeting  (volume_name fn: "rooz-{sanitize(key)}-{sanitize(entry)}")
    // unique_key output is already alphanumeric+dashes so sanitize is a no-op.
    let data_vol = format!("rooz-{}-greeting", key);

    let cfg = format!(
        "data:\n  greeting:\n    content: \"hello from rooz\\n\"\nmounts:\n  ~/greeting: greeting\n"
    );
    let cfg_path = format!("/tmp/rooz-test-{}.yaml", key);
    {
        let mut f = fs::File::create(&cfg_path).expect("write config");
        f.write_all(cfg.as_bytes()).unwrap();
    }

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args([
            "new",
            &key,
            "--image",
            "alpine:latest",
            "--config",
            &cfg_path,
        ])
        .assert()
        .success();

    assert!(
        env.volume_exists(&data_vol).await,
        "data volume {} not found",
        data_vol
    );

    // Content is written at greeting.data inside the volume (shadow-path convention).
    let content = env.volume_file(&data_vol, "greeting.data").await;
    assert_eq!(
        content.trim(),
        "hello from rooz",
        "unexpected content in data volume: {:?}",
        content
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    let _ = fs::remove_file(&cfg_path);
}

fn write_cfg(key: &str, yaml: &str) -> String {
    let path = format!("/tmp/rooz-test-{}.yaml", key);
    let mut f = fs::File::create(&path).expect("write config");
    f.write_all(yaml.as_bytes()).unwrap();
    path
}

fn new_workspace(env: &TestEnv, key: &str, cfg_path: &str) {
    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", key, "--image", "alpine:latest", "--config", cfg_path])
        .assert()
        .success();
}

fn cleanup(env: &TestEnv, key: &str, cfg_path: &str) {
    env.rooz().args(["rm", key, "--force"]).assert().success();
    let _ = fs::remove_file(cfg_path);
}

// ── v2: data-entry population (populate_volume / ensure_file_v2) ────────────

#[tokio::test]
async fn inline_mounts_share_inline_volume() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("vol-multi");
    let cfg_path = write_cfg(
        &key,
        "mounts:\n  ~/.cfg-a:\n    content: \"content-a\\n\"\n  ~/.cfg-b:\n    content: \"content-b\\n\"\n",
    );

    new_workspace(&env, &key, &cfg_path);

    // Both inline mounts land in the shared inline volume; file names are
    // sanitized target paths (shadow-path convention).
    let inline_vol = format!("rooz-{}-inline", key);
    assert_eq!(
        env.volume_file(&inline_vol, "---cfg-a.data").await,
        "content-a\n"
    );
    assert_eq!(
        env.volume_file(&inline_vol, "---cfg-b.data").await,
        "content-b\n"
    );

    cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn data_file_modes_ownership_and_eols() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("vol-attr");
    let cfg_path = write_cfg(
        &key,
        "data:\n  script:\n    content: \"#!/bin/sh\\necho hi\\n\"\n    executable: true\n  plain:\n    content: \"line1\\n\\nline3\\n\"\nmounts:\n  ~/script: script\n  ~/plain: plain\n",
    );

    new_workspace(&env, &key, &cfg_path);

    let script_vol = format!("rooz-{}-script", key);
    let plain_vol = format!("rooz-{}-plain", key);

    // executable entries get +x; owner is the workspace uid (default 1000).
    // Pins current behavior: the populate one-shot runs with umask 000, so
    // files come out world-writable (666/777 instead of 644/755).
    assert_eq!(
        env.volume_stat(&script_vol, "script.data").await,
        "777 1000"
    );
    assert_eq!(env.volume_stat(&plain_vol, "plain.data").await, "666 1000");

    // content must round-trip byte-exact: empty lines and the trailing EOL preserved
    assert_eq!(
        env.volume_file(&plain_vol, "plain.data").await,
        "line1\n\nline3\n"
    );

    cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn generated_data_file_content() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("vol-gen");
    let cfg_path = write_cfg(
        &key,
        "data:\n  gen:\n    generate: printf 'gen-output'\nmounts:\n  ~/gen: gen\n",
    );

    new_workspace(&env, &key, &cfg_path);

    let gen_vol = format!("rooz-{}-gen", key);
    assert_eq!(env.volume_file(&gen_vol, "gen.data").await, "gen-output");

    cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn generated_multiline_data_file() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("vol-genml");
    let cfg_path = write_cfg(
        &key,
        "data:\n  genml:\n    generate: printf 'l1\\nl2\\n'\nmounts:\n  ~/genml: genml\n",
    );

    new_workspace(&env, &key, &cfg_path);

    let vol = format!("rooz-{}-genml", key);
    let content = env.volume_file(&vol, "genml.data").await;
    // Pins current behavior: generator output is captured through a tty exec
    // (LF becomes CRLF) and trimmed (trailing EOL lost). Inline content is
    // preserved byte-exact; generated content is not.
    assert_eq!(content, "l1\r\nl2");

    cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn sidecar_mount_populates_data_volume() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("vol-sc");
    let cfg_path = write_cfg(
        &key,
        "data:\n  svc-cfg:\n    content: \"svc-config\\n\"\nsidecars:\n  svc:\n    image: alpine:latest\n    command:\n      - sleep\n    args:\n      - infinity\n    mounts:\n      /etc/svc-cfg: svc-cfg\n",
    );

    new_workspace(&env, &key, &cfg_path);

    let vol = format!("rooz-{}-svc-cfg", key);
    assert_eq!(env.volume_file(&vol, "svc-cfg.data").await, "svc-config\n");

    cleanup(&env, &key, &cfg_path);
}

// Reproduces the argv-size limit (E2BIG): file content travels base64-encoded
// inside a single `sh -c` argument, capped at 128KiB by the kernel
// (MAX_ARG_STRLEN). The generator keeps the config body itself small so this
// exercises only the v2 populate path.
#[tokio::test]
#[ignore = "known failure: E2BIG on large content — unignore with the put_archive migration"]
async fn large_generated_data_file() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("vol-big2");
    let cfg_path = write_cfg(
        &key,
        "data:\n  big:\n    generate: head -c 200000 /dev/zero | tr '\\0' x\nmounts:\n  ~/big: big\n",
    );

    new_workspace(&env, &key, &cfg_path);

    let vol = format!("rooz-{}-big", key);
    let content = env.volume_file(&vol, "big.data").await;
    assert_eq!(content.len(), 200000);
    assert!(content.chars().all(|c| c == 'x'));

    cleanup(&env, &key, &cfg_path);
}

// ── v1: RoozVolume population (ensure_mounts / ensure_file) ─────────────────

#[tokio::test]
async fn workspace_config_volume_stores_body() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("vol-cfgv1");
    let body = "image: alpine:latest\n# a trailing comment\n";
    let cfg_path = write_cfg(&key, body);

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let vol = format!("rooz-{}-workspace-config", key);
    let stored = env.volume_file(&vol, "workspace.config").await;
    // pins current v1 behavior: the body is trimmed before storing
    assert_eq!(stored, body.trim());

    // Pins current behavior: root-owned and, due to umask 000 in the populate
    // one-shot, world-writable.
    let stat = env.volume_stat(&vol, "workspace.config").await;
    assert_eq!(stat, "666 0");

    cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn system_config_volume_stores_rooz_config() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();

    let stored = env.volume_file("rooz_sys-config", "rooz.config").await;
    assert!(
        !stored.trim().is_empty(),
        "rooz.config missing or empty in the system config volume"
    );
}

// Reproduces the user-facing E2BIG on `populate volume: rooz-*-workspace-config`:
// the whole config body is base64-encoded into a single `sh -c` argument.
#[tokio::test]
#[ignore = "known failure: E2BIG on large config body — unignore with the put_archive migration"]
async fn large_workspace_config_body() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("vol-big1");
    let body = format!(
        "image: alpine:latest\n{}",
        "# padding padding padding padding padding\n".repeat(5000)
    );
    let cfg_path = write_cfg(&key, &body);

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let vol = format!("rooz-{}-workspace-config", key);
    let stored = env.volume_file(&vol, "workspace.config").await;
    assert_eq!(stored, body.trim());

    cleanup(&env, &key, &cfg_path);
}
