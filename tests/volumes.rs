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
