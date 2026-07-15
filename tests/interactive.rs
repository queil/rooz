mod harness;

use assert_cmd::cargo::cargo_bin;
use harness::{TestEnv, unique_key};
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

fn write_cfg(key: &str, yaml: &str) -> String {
    let path = format!("/tmp/rooz-test-{}.yaml", key);
    let mut f = fs::File::create(&path).expect("write config");
    f.write_all(yaml.as_bytes()).unwrap();
    path
}

fn cleanup(env: &TestEnv, key: &str, cfg_path: &str) {
    env.rooz().args(["rm", key, "--force"]).assert().success();
    let _ = fs::remove_file(cfg_path);
}

fn has_script() -> bool {
    Command::new("script")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Runs rooz under a real PTY (via util-linux `script`), feeding `input` to the
/// terminal. Returns (combined pty output, success).
fn pty_rooz(env: &TestEnv, args: &[&str], input: &str) -> (String, bool) {
    let cmd_line = format!(
        "env DOCKER_HOST={} HTTP_PROXY= http_proxy= {} {}",
        env.docker_host,
        cargo_bin("rooz").display(),
        args.join(" ")
    );
    let mut child = Command::new("timeout")
        .args(["120", "script", "-qec", &cmd_line, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn script");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait for script");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.success(),
    )
}

// ── install steps (non-interactive exec during rooz new) ────────────────────

#[tokio::test]
async fn install_steps_run_in_order() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("int-inst");
    let cfg_path = write_cfg(
        &key,
        "image: alpine:latest\ninstall:\n  01-first: echo first > /work/install-marker\n  02-second: echo second >> /work/install-marker\n",
    );

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    // headless on purpose: install must not require a controlling terminal
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let work_vol = format!("rooz-{}-work", key);
    assert_eq!(
        env.volume_file(&work_vol, "install-marker").await,
        "first\nsecond\n",
        "install steps did not run in order"
    );

    cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn failing_install_step_fails_new() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("int-fail");
    let cfg_path = write_cfg(
        &key,
        "image: alpine:latest\ninstall:\n  01-boom: \"exit 7\"\n",
    );

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();

    let output = env
        .rooz()
        .args(["new", &key, "--config", &cfg_path])
        .output()
        .expect("run rooz new");

    assert!(
        !output.status.success(),
        "rooz new must fail when an install step fails"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("install step '01-boom' failed"),
        "unexpected error output: {}",
        stderr
    );

    cleanup(&env, &key, &cfg_path);
}

// ── enter (interactive tty exec) ─────────────────────────────────────────────

#[tokio::test]
async fn enter_runs_interactive_shell() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    if !has_script() {
        eprintln!("skipping: util-linux 'script' not available for pty allocation");
        return;
    }
    let key = unique_key("int-enter");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();

    // the marker is computed in the shell so a match proves the command ran
    // (the typed command line echoes back on the pty as well)
    let (output, success) = pty_rooz(
        &env,
        &["enter", &key, "--shell", "sh"],
        "echo pty-marker-$((6*7))\nexit\n",
    );

    assert!(success, "rooz enter failed, pty output:\n{}", output);
    assert!(
        output.contains("pty-marker-42"),
        "shell did not evaluate the marker command, pty output:\n{}",
        output
    );

    env.rooz().args(["rm", &key, "--force"]).assert().success();
}
