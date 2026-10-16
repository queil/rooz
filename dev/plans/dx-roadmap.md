# DX roadmap

Agreed items, in suggested order. Status as of 0.164.0 work.

## Done

- **Per-step install guards, user default** - shipped 0.163.0. Steps run as the workspace user;
  `root: true` opts in; `skip_if` skips on exit 0. Pins `creates:` as rejected (we rarely know what a
  script creates).
- **Visible test skips** - branch `test-skips`. `SKIPPED (not run against an engine)` past libtest's
  capture; `ROOZ_TEST_REQUIRE_ENGINE=1` makes it fatal (set it in CI when wanted).
- **`--dry-run`** (merges "config check" and "plan") - branch `dry-run`, 0.164.0. `new --dry-run`,
  `update [--purge] --dry-run`. Same gates as a run; env/scripts/secrets never printed.

## Next

1. **Terse errors.** What / why / fix, compressed. Start with the panics:
   - failed image pull panics (`api/image.rs` pull), and leaves a half-created workspace behind;
   - `ensure_volume`, `create_volume`, `remove_volume` panic on unexpected engine errors;
   - `exec.rs` `panic!("Could not start exec")`.
2. **Phase progress, `-v`.** pull / volumes / each install step / clone; failing step prints exit code and
   log tail. `-v` replaces `RUST_LOG`.
3. **`rooz exec <ws> -- cmd`.** One command, exit code returned. Machinery exists in `ExecApi`.
4. **`rooz list` / `logs` / `status`.** Status, image, age, origin, `--json`; `logs <ws> [--container]`.
5. **`rooz cache ls|rm|prune`.** Size, last use, mounting workspaces. Cache volumes are
   `rooz_cache_<scope>_<path>`.
6. **`rooz doctor`, shell completions.** doctor: engine reachable (rootless/podman), `system init` done,
   ssh key, age identity, network address-pool limit.
7. **Config discoverability.** `rooz config schema` (JSON Schema for editors); `config show --resolved`
   with the layer each field came from.
8. **`rooz move`.** See `move.md`. Stage 1 (rename on one engine) shipped; stage 2 (cross-host) open.
9. **`rooz config template --detect`.** `config template` exists; detect `*.csproj`, `Cargo.toml`,
   fill image and caches.

## Ideas, not scheduled

- `rooz up <ws> --config f`: create if absent, update if present, never replace. Only if scripting/CI
  needs it; `new`/`update` stay the intent-explicit pair, replacement stays an explicit `rm`.
- `--dry-run --json` (the `Plan` struct is ready for it).
- Image layers per install step (commit after each step, keyed by base image id + steps so far). Removes
  the need for prebuilt images; needs layer naming, GC and the sidecar "trust the recorded image id"
  rule extended to a shared namespace. Large.
- Cross-host `move` with a direct host-to-host pipe instead of through the operator's machine.

## Refactors left from the duplication scan

- `get_cli_config` (`cmd/new.rs`): shared `load_local`; trust-gating code, add a test first.
- `container.rs` / `image.rs`: shared "inspect, tolerate 404 / stopped" and engine-error formatting.
  Decide panic-vs-error behaviour explicitly.
- `config.rs`: `render_indexmap`, generic `merge_opt`. Keep the `from_config` field list explicit (compile
  time exhaustiveness is deliberate).
- `runtime.rs`: `Masker` for the `mask_*` family; shared `validated_env`.
- `exec.rs`: `start_attached` prologue shared by `output` / `run` / `status`.
- Test harness: `skip_without_engine!` macro was rejected (hides the skip behaviour); remaining helpers
  (`label_filter`, `run_to_completion`) are fine.

## Open follow-ups

- `examples/cache-install.rooz.yaml`: downloads inside a container untested (no egress from `dkr`).
  Mount order, `HOME`, cache ownership and `skip_if` are verified.
- Non-root install `PATH` on podman: untested (docker only).
