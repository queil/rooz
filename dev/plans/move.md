# `rooz move`

Stage 1: rename a workspace on the same engine. Stage 2: move it to another host. Stage 2 reuses
stage 1's re-key logic and adds a streaming copy.

## UX

```
rooz move <ws> <new-name>            # stage 1
rooz move <ws> [<new-name>] --to <host> [--from <host>] [--with-caches] [--remove-source]   # stage 2
```

- `new`/`update` split stays: `move` never replaces. It refuses if the destination workspace exists.
- Stage 1 removes the old workspace after the new one is up (that is what a rename is). Stage 2 leaves the
  source stopped unless `--remove-source`; removal stays an explicit `rm`.
- `--dry-run` prints the volumes to copy and the `Plan` for the new key.

## Why a copy

Docker volumes cannot be renamed, and the workspace key is baked into volume names
(`rooz_<key>_work`, data volumes), network names, container names, `dev.rooz.workspace` labels and sidecar
image tags. So: copy data into volumes under the new key, rebuild the rest from the stored config.

## Stage 1 flow

1. Validate: `id::validate_workspace_name(new)`; source exists; destination absent (config volume by
   `Labels::workspace_config`).
2. Stop the workspace (`stop`).
3. Pair volumes: `create_volume_specs(old_key, ...)` vs `(new_key, ...)` from the same `RuntimeConfig`,
   matched by `DataEntryKey`; plus the work volume. Config volume and caches are not copied: caches are not
   workspace-labelled; config is rewritten by step 4.
4. For each pair: `ensure_volume(new_spec)` (labels right), then copy via a one-shot root container with
   both volumes mounted: `cp -a /from/. /to/`. Same engine, so no streaming.
5. Rebuild: the `update` path under the new key from the stored config (`ConfigSource::Update`), like
   `update` does for config-less workspaces (`None`). The root repo is not re-cloned (the clone line skips an
   existing `.git`; assert this in a test). New volumes already exist, so `populate_volume` is skipped
   (it only runs on `Created`).
6. On success `remove` the old workspace (volumes, config, containers, sidecar images). On any failure
   remove the new partial workspace and leave the old one untouched.

## Code touchpoints

- `cmd/move.rs` (new): orchestration. `api/volume.rs`: `copy_volume(from, to)` using
  `ContainerApi::one_shot_output`. `cli.rs`, `main.rs`: `Move(MoveParams)`.
- Reuse `WorkspaceApi::update`/`new_inner` rather than forking them; `update` needs a way to run under a
  different key than the stored one.
- Docs: README, CHANGELOG. Estimate ~250 lines plus tests.

## Tests (single engine, `dkr` is enough)

- Write a file in the work volume and in a data volume; `move`; assert file contents, volume labels,
  new names present, old names gone, sidecar running, config readable (`config show`).
- Failure path: force the copy to fail; assert old workspace intact and no `new` leftovers.
- Destination exists / invalid name / source missing: refused before anything is touched.
- Config-less workspace (`--image` only).

## Caveats

- Peak disk is roughly 2x the workspace data until step 6.
- Install steps and sidecar install images rebuild under the new name; downloads repeat unless cached or
  guarded by `skip_if`.
- Running processes and shell sessions end (the workspace is stopped).

## Stage 2 (cross-host) additions

- Two engine clients at once (`Api` is built around one). `rooz remote` / `-H` already resolve hosts and
  forward sockets; both hosts must be connected.
- Streaming copy: tar via bollard `download_from_container` into `upload_to_container`, no buffering
  (work volumes can be GBs). Preserves uid/mode/symlinks. Helper containers run as root on both sides.
- Target needs its own `rooz system init`: the ssh key pair is per engine by design. Register the new
  public key with git hosts. Do not copy the key between hosts. The age identity is on the operator's
  machine, so encrypted config still decrypts.
- Caches, the ssh key volume and install images are not moved (`--with-caches` optional).
- Architecture mismatch (x86 vs arm) breaks compiled artifacts in the work volume: warn, do not block.
- Transfer goes through the operator's machine (two hops); a direct host-to-host pipe over ssh is a later
  optimisation.
- Failure: delete partial target volumes; the source is never modified until `--remove-source`.
- Testing needs two daemons; CI boots one via `scripts/test-daemon.sh` (do not touch the workflow without
  asking). Locally, test the streaming primitive volume-to-volume on one engine.
