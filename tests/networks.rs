mod harness;

use harness::{TestEnv, unique_key};
use std::{fs, io::Write};

fn hub_spoke_cfg(key: &str) -> String {
    let path = format!("/tmp/rooz-test-networks-{}.yaml", key);
    let yaml = "\
image: alpine:latest
sidecars:
  claude:
    image: alpine:latest
    command: [sleep]
    args: [infinity]
    peers: [proxy]
  proxy:
    image: alpine:latest
    command: [sleep]
    args: [infinity]
    egress: true
  dkr:
    image: alpine:latest
    command: [sleep]
    args: [infinity]
    peers: [images]
  images:
    image: alpine:latest
    command: [sleep]
    args: [infinity]
    egress: true
";
    let mut f = fs::File::create(&path).expect("write networks config");
    f.write_all(yaml.as_bytes()).unwrap();
    path
}

#[tokio::test]
async fn hub_and_spoke_topology() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("net");
    let cfg_path = hub_spoke_cfg(&key);

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let networks = env.network_names_by_workspace(&key).await;
    let expected = {
        let mut v = vec![
            format!("{}-egress", key),
            format!("{}-net-claude", key),
            format!("{}-net-dkr", key),
            format!("{}-net-images", key),
            format!("{}-net-proxy", key),
            format!("{}-peer-claude-proxy", key),
            format!("{}-peer-dkr-images", key),
        ];
        v.sort();
        v
    };
    assert_eq!(networks, expected, "unexpected workspace network set");

    for sidecar in ["claude", "proxy", "dkr", "images"] {
        assert_eq!(
            env.ping(&key, sidecar).await,
            0,
            "main cannot reach sidecar '{}'",
            sidecar
        );
    }

    let dkr = format!("{}-dkr", key);
    let claude = format!("{}-claude", key);

    assert_eq!(
        env.ping(&dkr, "images").await,
        0,
        "dkr cannot reach its declared peer 'images'"
    );
    assert_eq!(
        env.ping(&claude, "proxy").await,
        0,
        "claude cannot reach its declared peer 'proxy'"
    );

    assert_ne!(
        env.ping(&dkr, "proxy").await,
        0,
        "dkr must not reach 'proxy' (no peer declared)"
    );
    assert_ne!(
        env.ping(&dkr, "claude").await,
        0,
        "dkr must not reach 'claude' (no peer declared)"
    );
    assert_ne!(
        env.ping(&claude, "images").await,
        0,
        "claude must not reach 'images' (no peer declared)"
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();

    assert!(
        env.network_names_by_workspace(&key).await.is_empty(),
        "workspace networks remain after rm"
    );

    let _ = fs::remove_file(&cfg_path);
}

#[tokio::test]
async fn unknown_peer_rejected_at_create() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("net-bad");
    let cfg_path = format!("/tmp/rooz-test-networks-{}.yaml", key);
    let yaml = "\
image: alpine:latest
sidecars:
  dkr:
    image: alpine:latest
    peers: [bogus]
";
    {
        let mut f = fs::File::create(&cfg_path).expect("write config");
        f.write_all(yaml.as_bytes()).unwrap();
    }

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    let assert = env
        .rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        stderr.contains("unknown peer 'bogus'"),
        "unexpected stderr: {}",
        stderr
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    let _ = fs::remove_file(&cfg_path);
}
