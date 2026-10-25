mod harness;

use harness::{TestEnv, pinned_consent, unique_key, write_cfg};
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

// 'update' removes the sidecar container, the only record of the image rooz built for it. It must
// carry that record over and reuse the image while the install steps are unchanged; a changed
// step builds it again. The install container mounts the sidecar's volumes, so the step counts
// its own runs there.
#[tokio::test]
async fn update_reuses_the_sidecar_image_until_the_install_steps_change() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-reuse");
    let cfg_path = format!("/tmp/rooz-test-sidecar-reuse-{}.yaml", key);
    let cfg = |step: &str| {
        format!(
            "image: alpine:latest\ndata:\n  runs: {{}}\nsidecars:\n  svc:\n    image: alpine:latest\n    command:\n      - sleep\n    args:\n      - infinity\n    mounts:\n      /runs: runs\n    install:\n      01-count: {}\n",
            step
        )
    };
    let write = |yaml: String| fs::write(&cfg_path, yaml).expect("write sidecar config");
    let runs_vol = format!("rooz_{}_runs", key);
    let runs = || async { env.volume_file(&runs_vol, "runs").await.lines().count() };

    write(cfg("echo run >> /runs/runs"));
    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();
    assert_eq!(runs().await, 1);

    env.rooz()
        .args(["update", &key, "--no-pull"])
        .assert()
        .success();
    assert_eq!(
        runs().await,
        1,
        "an update with unchanged steps ran them again"
    );

    write(cfg("echo run >> /runs/runs && true"));
    env.rooz()
        .args(["update", &key, "--no-pull"])
        .assert()
        .success();
    assert_eq!(
        runs().await,
        2,
        "a changed step did not build the image again"
    );

    env.rooz()
        .args(["update", &key, "--no-pull", "--rebuild"])
        .assert()
        .success();
    assert_eq!(runs().await, 3, "--rebuild did not build the image again");

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    let _ = fs::remove_file(&cfg_path);
}

// 'update' pulls the work image up front; the sidecar's base image must be pulled too, or a
// floating tag never moves. Here the tag is moved locally: the pull puts it back, so the base id
// and with it the build key are unchanged and the built image is reused, not rebuilt.
#[tokio::test]
async fn update_pulls_the_sidecar_base_image() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-pull");
    let cfg_path = format!("/tmp/rooz-test-sidecar-pull-{}.yaml", key);
    fs::write(
        &cfg_path,
        "image: alpine:latest\ndata:\n  runs: {}\nsidecars:\n  svc:\n    image: alpine:3\n    command:\n      - sleep\n    args:\n      - infinity\n    mounts:\n      /runs: runs\n    install:\n      01-count: echo run >> /runs/runs\n",
    )
    .expect("write sidecar config");
    let runs_vol = format!("rooz_{}_runs", key);

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path, "--pull-image"])
        .assert()
        .success();
    assert_eq!(env.volume_file(&runs_vol, "runs").await.lines().count(), 1);

    // any image other than alpine's stands in for the tag moving
    env.ensure_image("docker.io/chainguard/git:latest-dev")
        .await;
    env.tag_image("docker.io/chainguard/git:latest-dev", "alpine", "3")
        .await;
    env.rooz().args(["update", &key]).assert().success();
    assert_eq!(
        env.volume_file(&runs_vol, "runs").await.lines().count(),
        1,
        "update did not pull the sidecar's base image back"
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    let _ = fs::remove_file(&cfg_path);
}

// Sidecar images build alongside the work container's install, so their output interleaves: every
// line carries the tag of the container and step it came from.
#[tokio::test]
async fn install_output_is_tagged_per_container_and_step() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-tags");
    let cfg_path = format!("/tmp/rooz-test-sidecar-tags-{}.yaml", key);
    fs::write(
        &cfg_path,
        "image: alpine:latest\ninstall:\n  10-work: echo from-work\nsidecars:\n  svc:\n    image: alpine:latest\n    command:\n      - sleep\n    args:\n      - infinity\n    install:\n      10-svc: printf 'one\\ntwo'\n",
    )
    .expect("write sidecar config");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    let out = env
        .rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout).to_string();
    for line in [
        format!("[install:{}:10-work] from-work", key),
        "[install:svc:10-svc] one".to_string(),
        // a last line without a newline still comes out whole and tagged
        "[install:svc:10-svc] two".to_string(),
    ] {
        assert!(
            stdout.lines().any(|l| l == line),
            "missing '{}' in:\n{}",
            line,
            stdout
        );
    }

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    let _ = fs::remove_file(&cfg_path);
}

// Workspace 'a' with sidecar 'x' and workspace 'a-x' used to share the container name 'a-x', and
// the second workspace silently adopted the first one's container.
#[tokio::test]
async fn workspaces_whose_names_overlap_keep_their_own_containers() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let a = unique_key("sc-ovl");
    let a_x = format!("{}-x", a);
    let cfg_path = format!("/tmp/rooz-test-sidecar-ovl-{}.yaml", a);
    fs::write(
        &cfg_path,
        "image: alpine:latest\nsidecars:\n  x:\n    image: alpine:latest\n    command:\n      - sleep\n    args:\n      - infinity\n",
    )
    .expect("write sidecar config");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &a_x, "--image", "alpine:latest"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &a, "--config", &cfg_path])
        .assert()
        .success();

    let role = |c: &bollard_stubs::models::ContainerSummary| {
        c.labels
            .as_ref()
            .and_then(|l| l.get("dev.rooz.role"))
            .cloned()
            .unwrap_or_default()
    };
    let roles = |cs: Vec<bollard_stubs::models::ContainerSummary>| {
        let mut r = cs.iter().map(role).collect::<Vec<_>>();
        r.sort();
        r
    };
    assert_eq!(
        roles(env.containers_by_workspace(&a).await),
        vec!["sidecar", "work"]
    );
    assert_eq!(roles(env.containers_by_workspace(&a_x).await), vec!["work"]);

    // removing one leaves the other whole
    env.rooz().args(["rm", &a, "--force"]).assert().success();
    assert_eq!(roles(env.containers_by_workspace(&a_x).await), vec!["work"]);

    env.rooz().args(["rm", &a_x, "--force"]).assert().success();
    let _ = fs::remove_file(&cfg_path);
}

// A container rooz did not create for this workspace is never adopted under its name.
#[tokio::test]
async fn a_foreign_container_under_a_sidecar_name_is_refused() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-foreign");
    let cfg_path = format!("/tmp/rooz-test-sidecar-foreign-{}.yaml", key);
    fs::write(
        &cfg_path,
        "image: alpine:latest\nsidecars:\n  svc:\n    image: alpine:latest\n    command:\n      - sleep\n    args:\n      - infinity\n",
    )
    .expect("write sidecar config");
    let decoy = format!("{}_svc", key);
    env.create_decoy_container(&decoy).await;

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    let out = env
        .rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).to_string();
    assert!(stderr.contains("is not this workspace's"), "{}", stderr);
    assert!(
        env.container_exists(&decoy).await,
        "the foreign container was removed"
    );

    env.remove_decoy_container(&decoy).await;
    let _ = env.rooz().args(["rm", &key, "--force"]).output();
    let _ = fs::remove_file(&cfg_path);
}

// One container's failure must not cut the others short: a sidecar build stopped midway left its
// install container behind - base image, no env, none of the steps' work - posing as the sidecar.
#[tokio::test]
async fn a_failed_install_cuts_no_other_container_short() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-nocut");
    let cfg_path = format!("/tmp/rooz-test-sidecar-nocut-{}.yaml", key);
    fs::write(
        &cfg_path,
        "image: alpine:latest\ninstall:\n  10-fail: exit 3\nsidecars:\n  ok:\n    image: alpine:latest\n    command:\n      - sleep\n    args:\n      - infinity\n    install:\n      10-slow: sleep 3; touch /built\n  bad:\n    image: alpine:latest\n    command:\n      - sleep\n    args:\n      - infinity\n    install:\n      10-slow-fail: sleep 1; exit 4\n",
    )
    .expect("write sidecar config");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    let out = env
        .rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).to_string();
    assert!(stderr.contains("sidecar 'bad'"), "{}", stderr);
    assert!(stderr.contains("'10-fail'"), "{}", stderr);

    let containers = env.containers_by_workspace(&key).await;
    let named = |n: &str| {
        containers.iter().find(|c| {
            c.names
                .as_ref()
                .is_some_and(|ns| ns.iter().any(|x| x == &format!("/{}_{}", key, n)))
        })
    };
    assert!(
        named("bad").is_none(),
        "the failed sidecar's install container was left behind"
    );
    let ok = named("ok").expect("the sidecar that built fine was not created");
    // the image it runs, by id: the listed image name is the engine's to choose (docker shows the
    // reference rooz created it from, podman the image's name)
    let recorded = ok
        .labels
        .as_ref()
        .and_then(|l| l.get("dev.rooz.runtime-image"))
        .cloned();
    assert!(
        ok.image_id
            .as_deref()
            .zip(recorded.as_deref())
            .is_some_and(
                |(id, rec)| id.trim_start_matches("sha256:") == rec.trim_start_matches("sha256:")
            ),
        "the sidecar does not run its built image: {:?} vs {:?}",
        ok.image_id,
        recorded
    );

    let _ = env.rooz().args(["rm", &key, "--force"]).output();
    let _ = fs::remove_file(&cfg_path);
}

// ── vpn ───────────────────────────────────────────────────────────────────────

fn vpn_sidecar_cfg(key: &str) -> String {
    let path = format!("/tmp/rooz-test-sidecar-vpn-{}.yaml", key);
    let yaml = "\
image: alpine:latest
sidecars:
  wg:
    image: alpine:latest
    vpn: true
    command:
      - sleep
    args:
      - infinity
  plain:
    image: alpine:latest
    command:
      - sleep
    args:
      - infinity
";
    let mut f = fs::File::create(&path).expect("write sidecar config");
    f.write_all(yaml.as_bytes()).unwrap();
    path
}

#[tokio::test]
async fn vpn_sidecar_is_refused_without_consent() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-vpn-refused");
    let cfg_path = vpn_sidecar_cfg(&key);

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    let out = env
        .rooz()
        .args(["new", &key, "--config", &cfg_path])
        .env_remove("ROOZ_ALLOW_VPN")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "vpn sidecar created without consent");
    assert!(
        stderr.contains("ROOZ_ALLOW_VPN=wg@"),
        "refusal does not name the remedy: {}",
        stderr
    );
    assert!(
        env.containers_by_workspace(&key).await.is_empty(),
        "refused workspace left containers behind"
    );
    assert!(
        env.volumes_by_workspace(&key).await.is_empty(),
        "refused workspace left volumes behind"
    );

    // a bare name is not enough: consent is pinned to the sidecar's definition
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .env("ROOZ_ALLOW_VPN", "wg")
        .assert()
        .failure();

    // the same command goes through with the pinned consent the refusal names
    let allow = pinned_consent(&env, &key, &cfg_path, "ROOZ_ALLOW_VPN");
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .env("ROOZ_ALLOW_VPN", &allow)
        .assert()
        .success();

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    let _ = fs::remove_file(&cfg_path);
}

#[tokio::test]
async fn vpn_sidecar_gets_net_admin_and_tun() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-vpn");
    let cfg_path = vpn_sidecar_cfg(&key);

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    let allow = pinned_consent(&env, &key, &cfg_path, "ROOZ_ALLOW_VPN");
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .env("ROOZ_ALLOW_VPN", &allow)
        .assert()
        .success();

    let wg = format!("{}_wg", key);
    let plain = format!("{}_plain", key);
    let net_admin = vec!["ip", "link", "add", "t0", "type", "dummy"];
    let tun = vec!["test", "-c", "/dev/net/tun"];
    let src_valid_mark = vec![
        "grep",
        "-qx",
        "1",
        "/proc/sys/net/ipv4/conf/all/src_valid_mark",
    ];

    assert_eq!(
        env.exec_code(&wg, net_admin.clone()).await,
        0,
        "no NET_ADMIN"
    );
    assert_eq!(env.exec_code(&wg, tun.clone()).await, 0, "no /dev/net/tun");
    assert_eq!(
        env.exec_code(&wg, src_valid_mark.clone()).await,
        0,
        "src_valid_mark not set"
    );

    assert_ne!(
        env.exec_code(&plain, net_admin).await,
        0,
        "plain sidecar has NET_ADMIN"
    );
    assert_ne!(
        env.exec_code(&plain, tun).await,
        0,
        "plain sidecar has /dev/net/tun"
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    let _ = fs::remove_file(&cfg_path);
}

// The proxy image: the released one rooz pins, or ROOZ_EGRESS_IMAGE for a joey build not released
// yet - the variable reaches rooz from the test's own environment.
#[tokio::test]
async fn an_egress_policy_puts_the_sidecar_behind_its_own_proxy() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("sc-egress");
    let cfg_path = write_cfg(
        &key,
        "image: alpine:latest\nsidecars:\n  agent:\n    image: alpine:latest\n    command: [sleep]\n    args: [infinity]\n    egress:\n      allow: [allowed.test]\n      inject:\n        - host: api.example.test\n          rules:\n            - set: {authorization: Bearer t0ken}\n",
    );

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let agent = format!("{}_agent", key);
    let sh = |script: &str| vec!["sh".to_string(), "-c".to_string(), script.to_string()];
    let run = |script: &'static str| {
        let agent = agent.clone();
        let env = &env;
        async move {
            let cmd = sh(script);
            env.exec_code(&agent, cmd.iter().map(String::as_str).collect())
                .await
        }
    };

    // pointed at its proxy, which is up: a CA that failed to load would have stopped it
    assert_eq!(
        run("case \"$HTTPS_PROXY\" in http://rooz:*@agent-egress:9124) nc -z -w 5 agent-egress 9124 ;; *) exit 1 ;; esac")
            .await,
        0,
        "no proxy to reach"
    );
    // the proxy refuses a host outside the policy
    assert_eq!(
        run("t=${HTTPS_PROXY#http://rooz:}; t=${t%@*}; a=$(printf 'rooz:%s' \"$t\" | base64); printf 'CONNECT example.com:443 HTTP/1.1\\r\\nProxy-Authorization: Basic %s\\r\\n\\r\\n' \"$a\" | nc -w 5 agent-egress 9124 | head -1 | grep -q ' 403 '").await,
        0,
        "the proxy let an unlisted host through"
    );
    // the proxy acts only for the client given its credential: not the work container, which
    // reaches it too, nor the sidecar without the credential
    let unauthenticated = "printf 'CONNECT allowed.test:443 HTTP/1.1\\r\\n\\r\\n' | nc -w 5 agent-egress 9124 | head -1 | grep -q ' 407 '";
    assert_eq!(
        run(unauthenticated).await,
        0,
        "the proxy served a client without its credential"
    );
    let work = key.clone();
    let cmd = sh(unauthenticated);
    assert_eq!(
        env.exec_code(&work, cmd.iter().map(String::as_str).collect())
            .await,
        0,
        "the work container used the proxy"
    );
    // the sidecar trusts the proxy's CA and never sees its key
    assert_eq!(
        run("grep -q 'BEGIN CERTIFICATE' \"$NODE_EXTRA_CA_CERTS\" && ! grep -rqs 'PRIVATE KEY' /usr/local/share/ca-certificates /etc/joey").await,
        0,
        "CA certificate missing, or the key visible"
    );
    // and has no way out but the proxy
    assert_ne!(
        run("nc -z -w 3 1.1.1.1 443").await,
        0,
        "the sidecar reached the internet directly"
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();
    let _ = fs::remove_file(&cfg_path);
}
