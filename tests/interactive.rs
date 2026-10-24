mod harness;

use assert_cmd::cargo::cargo_bin;
use base64::{Engine, engine::general_purpose::STANDARD};
use harness::{TestEnv, cleanup, unique_key, write_cfg};
use std::{
    io::Write,
    process::{Command, Stdio},
};

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

    let work_vol = format!("rooz_{}_work", key);
    assert_eq!(
        env.volume_file(&work_vol, "install-marker").await,
        "first\nsecond\n",
        "install steps did not run in order"
    );

    cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn install_steps_run_as_user_unless_root_is_requested() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("int-inst-user");
    let cfg_path = write_cfg(
        &key,
        "image: alpine:latest\ninstall:\n  01-user: id -u > /work/uid-user\n  02-root:\n    run: id -u > /work/uid-root\n    root: true\n",
    );

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let work_vol = format!("rooz_{}_work", key);
    let user = env.volume_file(&work_vol, "uid-user").await;
    assert_ne!(user.trim(), "0", "default install step ran as root");
    assert_eq!(env.volume_file(&work_vol, "uid-root").await.trim(), "0");

    cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn install_skip_if_skips_on_zero_exit_only() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("int-inst-skip");
    let cfg_path = write_cfg(
        &key,
        "image: alpine:latest\ninstall:\n  01-skipped:\n    skip_if: 'true'\n    run: echo skipped >> /work/marker\n  02-runs:\n    skip_if: 'false'\n    run: echo ran >> /work/marker\n",
    );

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();

    let work_vol = format!("rooz_{}_work", key);
    assert_eq!(env.volume_file(&work_vol, "marker").await, "ran\n");

    cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn update_rebuild_runs_steps_whose_skip_if_succeeds() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("int-rebuild-skip");
    let cfg_path = write_cfg(
        &key,
        "image: alpine:latest\ninstall:\n  01-guarded:\n    skip_if: 'true'\n    run: echo ran >> /work/marker\n",
    );

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--config", &cfg_path])
        .assert()
        .success();
    let work_vol = format!("rooz_{}_work", key);
    assert_eq!(env.volume_file(&work_vol, "marker").await, "");

    env.rooz()
        .args(["update", &key, "--no-pull"])
        .assert()
        .success();
    assert_eq!(
        env.volume_file(&work_vol, "marker").await,
        "",
        "a plain update honours skip_if"
    );

    env.rooz()
        .args(["update", &key, "--no-pull", "--rebuild"])
        .assert()
        .success();
    assert_eq!(env.volume_file(&work_vol, "marker").await, "ran\n");

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
    assert!(
        stderr.contains(&format!("rooz update {}", key)),
        "no resume hint in: {}",
        stderr
    );

    cleanup(&env, &key, &cfg_path);
}

#[tokio::test]
async fn update_resumes_a_failed_install_from_its_checkpoint() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("int-ckpt");
    let steps = |second: &str| {
        format!(
            "image: alpine:latest\ninstall:\n  01-first:\n    run: echo ran >> /first-runs\n    root: true\n  02-second:\n    run: {}\n    root: true\n",
            second
        )
    };
    let cfg_path = write_cfg(&key, &steps("exit 7"));

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();

    let failed = env
        .rooz()
        .args(["new", &key, "--config", &cfg_path])
        .output()
        .expect("run rooz new");
    assert!(!failed.status.success(), "the failing step must fail new");

    let mut filters = std::collections::HashMap::new();
    filters.insert(
        "label".to_string(),
        vec![
            format!("dev.rooz.workspace={}", key),
            "dev.rooz.role=install-checkpoint".to_string(),
        ],
    );
    let checkpoints = env
        .docker
        .list_images(Some(bollard_stubs::query_parameters::ListImagesOptions {
            filters: Some(filters.clone()),
            all: true,
            ..Default::default()
        }))
        .await
        .unwrap();
    assert_eq!(
        checkpoints.len(),
        1,
        "the step that ran leaves one checkpoint"
    );

    write_cfg(&key, &steps("echo fixed > /second-ran"));
    env.rooz().args(["update", &key]).assert().success();

    assert_eq!(
        env.exec_code(
            &key,
            vec!["sh", "-c", "test \"$(wc -l < /first-runs)\" = 1"]
        )
        .await,
        0,
        "the first step ran again instead of being resumed"
    );
    assert_eq!(
        env.exec_code(&key, vec!["test", "-f", "/second-ran"]).await,
        0,
        "the second step did not run"
    );

    cleanup(&env, &key, &cfg_path);
}

// A remote --config must be ssh. This serves a repository over HTTP from the engine and routes
// `ssh://git@rooz-test.invalid/` to it through the system gitconfig; `teardown` undoes both.
struct ServedRepo {
    name: String,
    original_sys_config: String,
}

fn sys_config_write(body: &str) -> String {
    format!(
        "printf '%s' '{}' | base64 -d > /mnt/rooz.config",
        STANDARD.encode(body)
    )
}

// The server speaks dumb HTTP, which has no shallow clones, so the config clone (depth 1) fails
// and leaves an empty clone dir: enough to reach the config read after the clone, not to read one.
async fn serve_config_repo(env: &TestEnv, key: &str) -> ServedRepo {
    let name = format!("rooz-test-repo-{}", key);
    env.run_in_volume(
        "docker.io/chainguard/git:latest-dev",
        &name,
        "set -e; cd /mnt; git init -q -b main w; \
         git -C w -c user.email=t@t -c user.name=t commit -q --allow-empty -m repo; \
         git clone -q --bare w www/r.git; git -C www/r.git update-server-info",
    )
    .await;
    let ip = env.serve_dir(&name, &name, "/srv/www").await;

    let original_sys_config = env.volume_file("rooz_sys-config", "rooz.config").await;
    let route = format!(
        "  [url \"http://{}/\"]\n    insteadOf = ssh://git@rooz-test.invalid/\n",
        ip
    );
    let routed = if original_sys_config.contains("gitconfig:") {
        format!("{}\n{}", original_sys_config.trim_end(), route)
    } else {
        format!("{}gitconfig: |-\n{}", original_sys_config, route)
    };
    env.run_in_volume(
        "alpine:latest",
        "rooz_sys-config",
        &sys_config_write(&routed),
    )
    .await;
    ServedRepo {
        name,
        original_sys_config,
    }
}

async fn teardown(env: &TestEnv, repo: ServedRepo) {
    env.run_in_volume(
        "alpine:latest",
        "rooz_sys-config",
        &sys_config_write(&repo.original_sys_config),
    )
    .await;
    env.remove_decoy_container(&repo.name).await;
    env.remove_decoy_volume(&repo.name).await;
}

const SERVED_CONFIG: &str = "ssh://git@rooz-test.invalid/r.git//rooz.yaml";

#[tokio::test]
async fn a_failed_config_read_leaves_no_clone_container_behind() {
    let Some(env) = TestEnv::from_env() else {
        return;
    };
    if !has_script() {
        return;
    }
    let key = unique_key("int-cfg-read");
    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    let repo = serve_config_repo(&env, &key).await;

    let before = env.git_containers().await;
    let (out, ok) = pty_rooz(&env, &["new", &key, "--config", SERVED_CONFIG], "");
    let after = env.git_containers().await;

    teardown(&env, repo).await;
    env.rooz().args(["rm", &key, "--force"]).output().ok();

    // the clone worked and the read inside its container failed
    assert!(
        !ok,
        "a config that is not in the repository was found: {}",
        out
    );
    assert!(out.contains("not found or empty"), "{}", out);
    // and that container, which mounts the ssh key, is gone
    assert_eq!(
        after, before,
        "the clone container outlived the failed read"
    );
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

/// Reads the pty master until `needle` appears or `secs` pass.
fn read_pty_until(master: i32, needle: &str, secs: u64) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let mut out = Vec::new();
    while std::time::Instant::now() < deadline {
        let mut pfd = libc::pollfd {
            fd: master,
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut pfd, 1, 200) } <= 0 {
            continue;
        }
        let mut buf = [0u8; 4096];
        let n = unsafe { libc::read(master, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
        if String::from_utf8_lossy(&out).contains(needle) {
            break;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn set_pty_size(fd: i32, rows: u16, cols: u16) {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) }, 0);
}

#[tokio::test]
async fn enter_follows_terminal_resize() {
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;

    let Some(env) = TestEnv::from_env() else {
        return;
    };
    let key = unique_key("int-resize");

    env.rooz()
        .args(["system", "init", "--force"])
        .assert()
        .success();
    env.rooz()
        .args(["new", &key, "--image", "alpine:latest"])
        .assert()
        .success();

    let (mut master, mut slave) = (0, 0);
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0, "openpty failed");
    set_pty_size(master, 24, 80);

    let slave_fd = unsafe { OwnedFd::from_raw_fd(slave) };
    // same environment as TestEnv::rooz, on a plain Command so the pty can be wired in
    let mut cmd = Command::new(cargo_bin("rooz"));
    cmd.args(["enter", &key, "--shell", "sh"])
        .env("DOCKER_HOST", &env.docker_host)
        .env("http_proxy", "")
        .env("HTTP_PROXY", "")
        .env(
            "ROOZ_AGE_KEY_FILE",
            std::env::temp_dir()
                .join("rooz-test-identity")
                .join("age.key"),
        )
        .stdin(slave_fd.try_clone().unwrap())
        .stdout(slave_fd.try_clone().unwrap())
        .stderr(slave_fd.try_clone().unwrap());
    // rooz reads the size from /dev/tty, so the pty must be its controlling terminal
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY as _, 0);
            Ok(())
        });
    }
    let mut child = cmd.spawn().expect("spawn rooz enter");
    drop(slave_fd);

    let write = |s: &str| unsafe {
        libc::write(master, s.as_ptr().cast(), s.len());
    };
    // the marker is computed in the shell so the echoed command line does not match it.
    // Input typed before the session attaches is lost, and one typed while the shell is still
    // setting its terminal up can run before the size is known, so retry until the shell has
    // answered with the size
    let mut before = String::new();
    for _ in 0..30 {
        write("stty size; echo winsz-$((40+2))\n");
        before.push_str(&read_pty_until(master, "winsz-42", 2));
        if before.contains("winsz-42") && before.contains("24 80") {
            break;
        }
    }
    assert!(
        before.contains("24 80"),
        "initial size, pty output:\n{}",
        before
    );

    // the kernel sends SIGWINCH to the session's foreground process group
    set_pty_size(master, 40, 120);
    std::thread::sleep(std::time::Duration::from_secs(1));
    write("stty size; echo winsz-$((40+3))\n");
    let after = read_pty_until(master, "winsz-43", 30);
    assert!(
        after.contains("40 120"),
        "size after resize, pty output:\n{}",
        after
    );

    write("exit\n");
    let _ = child.wait();
    unsafe { libc::close(master) };

    env.rooz().args(["rm", &key, "--force"]).assert().success();
}
