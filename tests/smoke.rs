mod harness;

use harness::{TestEnv, unique_key};

#[tokio::test]
async fn smoke_new_and_rm() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };

    let key = unique_key("smoke");

    env.rooz().args(["system", "init"]).assert().success();

    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();

    let containers = env.containers_by_workspace(&key).await;
    assert!(!containers.is_empty(), "expected containers after rooz new");

    for c in &containers {
        let labels = c
            .labels
            .as_ref()
            .unwrap_or(&std::collections::HashMap::new())
            .clone();
        assert_eq!(
            labels.get("dev.rooz.workspace").map(String::as_str),
            Some(key.as_str()),
            "container missing workspace label"
        );
    }

    env.rooz().args(["rm", &key, "--force"]).assert().success();

    let containers_after = env.containers_by_workspace(&key).await;
    assert!(
        containers_after.is_empty(),
        "containers still present after rooz rm"
    );

    let volumes_after = env.volumes_by_workspace(&key).await;
    assert!(
        volumes_after.is_empty(),
        "volumes still present after rooz rm"
    );
}
