# DX roadmap

Agreed items, in suggested order. Status as of 0.166.0.

## Done

- **Per-step install guards, user default** - shipped 0.163.0. Steps run as the workspace user;
  `root: true` opts in; `skip_if` skips on exit 0. Pins `creates:` as rejected (we rarely know what a
  script creates).
- **Visible test skips** - branch `test-skips`. `SKIPPED (not run against an engine)` past libtest's
  capture; `ROOZ_TEST_REQUIRE_ENGINE=1` makes it fatal (set it in CI when wanted).
- **`--dry-run`** (merges "config check" and "plan") - branch `dry-run`, 0.164.0. `new --dry-run`,
  `update [--purge] --dry-run`. Same gates as a run; env/scripts/secrets never printed.
- **Terse errors, phase progress and `-v`, `exec`, `list`/`status`/`logs`, `cache`, `doctor`,
  `config schema`/`show --resolved`, `move` stage 1, `config template --detect`** - shipped 0.164.0, see
  CHANGELOG. Not done from these: a log tail for failing install steps (their output already streams
  live), "last use" for caches (the engine does not track it), hover descriptions in the config schema.

- **`rooz move --to <host>`** (stage 2) - shipped 0.164.0. Built as two ordinary rooz processes (`-H`
  source, `-H` target) with the tar streamed between them, so no second engine client exists in one
  process. Tested on one daemon behind a second unix socket; a real second host is not tested.
  Not done: `--with-caches`, `--from` (use `-H`), a direct host-to-host pipe.

## Next

Nothing scheduled.

## Ideas, not scheduled

- `rooz up <ws> --config f`: create if absent, update if present, never replace. Only if scripting/CI
  needs it; `new`/`update` stay the intent-explicit pair, replacement stays an explicit `rm`.
- `--dry-run --json` (the `Plan` struct is ready for it).
- Image layers per install step (commit after each step, keyed by base image id + steps so far). Removes
  the need for prebuilt images; needs layer naming, GC and the sidecar "trust the recorded image id"
  rule extended to a shared namespace. Large.
- Cross-host `move` with a direct host-to-host pipe instead of through the operator's machine;
  `--with-caches`; moving the workspace from its stored config when the config file is gone.

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
