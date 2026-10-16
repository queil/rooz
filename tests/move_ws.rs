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

// A second "host" that is the same engine behind a unix socket, which is all 'rooz remote'
// provides: enough to run the cross-host code path (two clients, a streamed copy) on the one
// daemon CI has. The target name differs because the engine is shared.
struct AltHost {
    dir: std::path::PathBuf,
}

impl AltHost {
    fn start(env: &TestEnv, tag: &str) -> AltHost {
        let dir = std::env::temp_dir().join(format!("rooz-alt-{}", tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("rooz")).unwrap();
        std::fs::write(
            dir.join("remotes.yaml"),
            "hosts:\n  alt:\n    ssh: unused\n",
        )
        .unwrap();

        let socket = dir.join("rooz").join("alt.sock");
        let upstream = env.docker_host.trim_start_matches("tcp://").to_string();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = tokio::net::UnixListener::bind(&socket).unwrap();
                ready_tx.send(()).unwrap();
                loop {
                    let (mut client, _) = listener.accept().await.unwrap();
                    let upstream = upstream.clone();
                    tokio::spawn(async move {
                        if let Ok(mut engine) = tokio::net::TcpStream::connect(upstream).await {
                            let _ = tokio::io::copy_bidirectional(&mut client, &mut engine).await;
                        }
                    });
                }
            });
        });
        ready_rx.recv().unwrap();
        AltHost { dir }
    }

    fn rooz(&self, env: &TestEnv) -> assert_cmd::Command {
        let mut cmd = env.rooz();
        cmd.env("XDG_RUNTIME_DIR", &self.dir)
            .env("ROOZ_REMOTES_FILE", self.dir.join("remotes.yaml"));
        cmd
    }
}

impl Drop for AltHost {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn move_to_a_host_streams_volumes_and_keeps_owners_and_modes() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let (from, to) = (unique_key("mv-g"), unique_key("mv-h"));
    let alt = AltHost::start(&env, &from);
    init(&env);
    env.rooz()
        .args(["new", &from, "--image", "alpine:latest"])
        .assert()
        .success();
    exec_out(
        &env,
        &from,
        "echo kept > /work/marker; touch /work/f; chown 1234:4321 /work/f; chmod 640 /work/f; ln -s marker /work/link",
    );

    alt.rooz(&env)
        .args(["move", &from, &to, "--to", "alt", "--dry-run"])
        .assert()
        .success();
    assert!(
        env.volumes_by_workspace(&to).await.is_empty(),
        "dry run copied"
    );

    alt.rooz(&env)
        .args(["move", &from, &to, "--to", "alt"])
        .assert()
        .success();

    assert_eq!(exec_out(&env, &to, "cat /work/marker").trim(), "kept");
    assert_eq!(
        exec_out(&env, &to, "stat -c '%u:%g %a' /work/f").trim(),
        "1234:4321 640"
    );
    assert_eq!(exec_out(&env, &to, "readlink /work/link").trim(), "marker");

    // the source is left in place, stopped
    assert!(!env.volumes_by_workspace(&from).await.is_empty());
    assert!(
        env.containers_by_workspace(&from)
            .await
            .iter()
            .all(|c| c.state != Some(bollard_stubs::models::ContainerSummaryStateEnum::RUNNING)),
        "source still running"
    );

    // a target that exists is refused before anything is touched
    alt.rooz(&env)
        .args(["move", &from, &to, "--to", "alt"])
        .assert()
        .failure();

    for k in [&from, &to] {
        env.rooz().args(["rm", k, "--force"]).assert().success();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn move_to_a_host_can_remove_the_source() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let (from, to) = (unique_key("mv-i"), unique_key("mv-j"));
    let alt = AltHost::start(&env, &from);
    let cfg_path = write_cfg(
        &from,
        "image: alpine:latest\ndata:\n  greeting:\n    content: \"hello\\n\"\nmounts:\n  ~/greeting: greeting\n",
    );
    init(&env);
    env.rooz()
        .args(["new", &from, "--config", &cfg_path])
        .assert()
        .success();

    alt.rooz(&env)
        .args(["move", &from, &to, "--to", "alt", "--remove-source"])
        .assert()
        .success();

    assert!(env.volumes_by_workspace(&from).await.is_empty());
    assert!(env.containers_by_workspace(&from).await.is_empty());
    assert_eq!(
        env.volume_file(&format!("rooz-{}-greeting", to), "greeting")
            .await,
        "hello\n"
    );

    cleanup(&env, &to, &cfg_path);
}
