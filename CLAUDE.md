# rooz dev workspace notes

## Environment

This workspace runs in a restricted container (rooz with podman backend):

- The Docker daemon is a dedicated sidecar at `tcp://dkr:2375` (the default `DOCKER_HOST`). It is this workspace's to use — nothing else runs on it, so integration tests target it directly.
- Egress goes through an allowlisting HTTP proxy (`HTTP_PROXY=http://proxy:9124`) which blocks `dkr`. Add `dkr` to `NO_PROXY` for docker CLI / curl. Bollard-based code (the test harness and rooz itself) ignores proxy vars and connects directly.
- There is no external DNS — all public lookups return NXDOMAIN. The `dkr` daemon can pull images only via its registry mirror `http://images:5000`. Any nested dind daemon must be started with `--registry-mirror=http://images:5000` or every pull fails.
- Environment restarts recreate `dkr` and wipe its image cache. Transient `Blocked: <host>` or DNS errors usually mean the environment is being reconfigured — ask, don't work around.

## Integration tests

Run locally, straight against `dkr` (no dind):

```sh
cargo test --test smoke --test lifecycle --test volumes --test sidecars --test networks -- --test-threads=1
```

This works because a gitignored `.cargo/config.toml` supplies the test env. Recreate it if missing:

```toml
[env]
ROOZ_TEST_DOCKER_HOST = "tcp://dkr:2375"
ROOZ_TEST_ENGINE = "docker"
```

- The lifecycle suite runs `rooz system prune`, which removes ALL rooz-labeled resources on the target daemon. Fine for `dkr`; don't point the tests at a daemon holding real rooz workspaces.
- CI (`.github/workflows/integration.yml`) boots its own isolated daemon via `scripts/test-daemon.sh` and sets `ROOZ_TEST_*` explicitly — leave it untouched. Cargo's `[env]` never overrides already-set variables, so the local config cannot leak into CI.
