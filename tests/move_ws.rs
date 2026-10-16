mod harness;

use harness::{TestEnv, cleanup, unique_key, write_cfg};

fn init(env: &TestEnv) {
    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
}

fn exec_out(env: &TestEnv, key: &str, script: &str) -> String {
    let out = env
        .rooz()
        .args(["exec", "--root", key, "--", "sh", "-c", script])
        .assert()
        .success();
    String::from_utf8_lossy(&out.get_output().stdout).to_string()
}

#[tokio::test]
async fn move_renames_a_workspace_made_from_flags_and_keeps_its_data() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let (from, to) = (unique_key("mv-a"), unique_key("mv-b"));
    init(&env);
    env.rooz()
        .args(["new", &from, "--image", "alpine:latest"])
        .assert()
        .success();
    exec_out(&env, &from, "echo kept > /work/marker");

    env.rooz().args(["move", &from, &to]).assert().success();

    assert!(env.containers_by_workspace(&from).await.is_empty());
    assert!(env.volumes_by_workspace(&from).await.is_empty());
    assert!(!env.volumes_by_workspace(&to).await.is_empty());
    for v in env.volumes_by_workspace(&to).await {
        assert_eq!(
            v.labels.get("dev.rooz.workspace").map(String::as_str),
            Some(to.as_str()),
            "volume {} not relabelled",
            v.name
        );
    }
    assert_eq!(exec_out(&env, &to, "cat /work/marker").trim(), "kept");

    let list = env.rooz().args(["list", "--json"]).assert().success();
    let rows: serde_json::Value = serde_json::from_slice(&list.get_output().stdout).unwrap();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == to.as_str())
        .expect("moved workspace listed");
    assert!(row["image"].as_str().unwrap().contains("alpine"));

    env.rooz().args(["rm", &to, "--force"]).assert().success();
}

#[tokio::test]
async fn move_carries_config_data_volumes_and_the_stored_config() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let (from, to) = (unique_key("mv-c"), unique_key("mv-d"));
    let cfg_path = write_cfg(
        &from,
        "image: alpine:latest\ndata:\n  greeting:\n    content: \"hello\\n\"\nmounts:\n  ~/greeting: greeting\n",
    );
    init(&env);
    env.rooz()
        .args(["new", &from, "--config", &cfg_path])
        .assert()
        .success();
    exec_out(&env, &from, "echo more >> /home/rooz_user/greeting");

    env.rooz().args(["move", &from, &to]).assert().success();

    assert!(env.volumes_by_workspace(&from).await.is_empty());
    let data = env
        .volume_file(&format!("rooz-{}-greeting", to), "greeting")
        .await;
    assert_eq!(data, "hello\nmore\n");

    let origin = env
        .rooz()
        .args(["config", "show", &to, "--part", "origin"])
        .assert()
        .success();
    assert!(
        String::from_utf8_lossy(&origin.get_output().stdout).contains(&cfg_path),
        "config origin lost"
    );

    cleanup(&env, &to, &cfg_path);
}

#[tokio::test]
async fn move_refuses_before_touching_anything() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let (a, b) = (unique_key("mv-e"), unique_key("mv-f"));
    init(&env);
    for k in [&a, &b] {
        env.rooz()
            .args(["new", k, "--image", "alpine:latest"])
            .assert()
            .success();
    }
    let before = env.volumes_by_workspace(&a).await.len();

    env.rooz().args(["move", &a, &b]).assert().failure();
    env.rooz().args(["move", &a, "Bad_Name"]).assert().failure();
    env.rooz().args(["move", &a, &a]).assert().failure();
    env.rooz()
        .args(["move", "no-such-workspace-xyz", "whatever"])
        .assert()
        .failure();
    env.rooz()
        .args(["move", &a, "mv-dry-target", "--dry-run"])
        .assert()
        .success();

    assert_eq!(env.volumes_by_workspace(&a).await.len(), before);
    assert!(env.volumes_by_workspace("mv-dry-target").await.is_empty());
    assert!(!env.volumes_by_workspace(&b).await.is_empty());

    for k in [&a, &b] {
        env.rooz().args(["rm", k, "--force"]).assert().success();
    }
}
