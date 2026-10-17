# Changelog

Notable changes per release. The release workflow refuses to publish a tag whose version has
no section here, and the section becomes the release notes - so write it before bumping
`Cargo.toml`, then rename `Unreleased` to the version you are tagging.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Releases before 0.159.0 predate this
file - see the git history.

## [0.168.0] - 2026-10-17

### Changed

- **Breaking:** `ports` mappings are `<host>:<container>`, as in Docker and `kubectl port-forward`
  (was `<container>:<host>`). Swap the numbers in existing configs: `"80:8080"` that exposed
  container port 80 on host port 8080 is now `"8080:80"`.
- **Breaking:** remote `--config` sources must use SSH. Non-SSH URLs (`https://`, `http://`,
  `git://`, `file://`, remote helpers) are refused with a clear message.
- **Breaking:** the sidecar `user` key is removed; it was accepted but never used. Configs that set it
  fail to parse - use `uid`.

### Added

- Remote `--config` accepts `ssh://` URLs (also `git+ssh://`, `ssh+git://`), e.g.
  `ssh://git@host:2222/my/configs//cfg.yaml` for a non-standard port.

### Fixed

- The examples in `examples/` parse again; a unit test keeps them that way.
- `rooz doctor`'s network hint counts networks correctly: one per workspace, plus one per sidecar and
  one per peer relation.
- The secrets refusal message showed a `--config git:...` syntax that does not exist.

## [0.167.0] - 2026-10-17

### Added

- Docs site at [rooz.dev](https://rooz.dev), built from `docs/` with mdBook, including a generated CLI
  reference and config schema.
- `curl -fsSL https://rooz.dev/install.sh | sh` installs the latest release. Each release carries an
  `install.sh` that pins its own version and `SHA256SUMS`, so the binary is checked against data
  served from rooz.dev, not the GitHub release it comes from. With a logged-in `gh`, it also checks
  the signed build provenance.

## [0.166.0] - 2026-10-16

### Changed

- **Breaking:** volume names use one scheme: segments joined by `_`, workspace first
  (`rooz_<ws>_work`, `rooz_<ws>_workspace-config`, `rooz_<ws>_<data>`; shared ones stay
  `rooz_sys-config`, `rooz_cache_<scope>_<path>`). The ssh key volume is `rooz_ssh-key`, was
  `rooz-ssh-key-vol`. Volumes under the old names are not found or migrated: remove them
  (`rooz system prune` removes everything rooz made), then run `rooz system init` - a new ssh key is
  generated and must be registered again.

## [0.165.0] - 2026-10-16

### Changed

- `rooz doctor` starts with the endpoint it connects to (the `-H` host's socket, `DOCKER_HOST`, or
  the default socket), and `-v` logs it for every command.

### Fixed

- An engine that cannot be reached is reported as `Cannot reach the container engine at <where>:
  <why>`, with what to check, instead of the HTTP client's `client error (Connect)`. A missing
  default socket (no `DOCKER_HOST`, no `/var/run/docker.sock`) no longer panics.

## [0.164.0] - 2026-10-16

### Added

- `rooz new --dry-run` and `rooz update --dry-run` (also with `--purge`) print what would be removed
  and created - image pull, volumes, caches, networks, sidecars, the work container and install steps
  - and change nothing. The configuration is resolved and gated like a real run, so a plan fails with
  the error a run would. Env values, scripts and secrets are not printed.
- `rooz exec <ws> -- cmd...` runs one command in a workspace container, passes stdout and stderr
  through and exits with the command's exit code. `--root`, `--work-dir`, `--container`.
- `rooz list --json`; `rooz status <ws> [--json]` (containers, volumes, config origin);
  `rooz logs <ws> [--container] [-f] [--tail N]`.
- `rooz cache ls|rm|prune`: size, age and mounting workspaces of the shared cache volumes. `rm` and
  `prune` only touch your own scope and refuse caches a container still mounts.
- `rooz doctor` checks the engine, `rooz system init` results (system config, ssh key, age identity)
  and, on docker, how much of the default network address pool is used.
- `rooz move <ws> <new-name> [--dry-run]` renames a workspace on one engine: volumes are copied under
  the new name, the workspace is rebuilt from its stored config and the old one is removed. A failure
  removes the partial new workspace and leaves the old one untouched.
- `rooz move <ws> --to <host> [new-name] [--remove-source] [--dry-run]` moves a workspace to another
  engine from `rooz remote`: volumes stream across with owners, modes and symlinks intact, the
  workspace is rebuilt there from its config file, and the source is left stopped unless
  `--remove-source`. The target is checked before anything is copied (name free, `system init` done,
  config gates) and a failure removes the partial workspace there.
- `rooz config schema` prints a JSON Schema of the config file for YAML language servers.
- `rooz config show <ws> --resolved` shows the merged settings (defaults, bases, config) with the
  layer each came from.
- `rooz config template` (it was a stub) prints a starter config; with `--detect [DIR]` it
  recognizes Rust, .NET, Node, Go and Python projects and fills in the image and cache paths.
- Global `-v/--verbose` turns on debug logging (`RUST_LOG` still works). `new` and `update` print one
  colored `[phase]` line per step: image, volumes, clone, container, install.
- `rooz update --allow-secrets`, as on `new`; before, only `ROOZ_ALLOW_SECRETS` could consent.

### Changed

- **Breaking:** `rooz list` shows STATUS, IMAGE and AGE instead of RUNNING, and includes workspaces
  whose containers are stopped.
- A failed image pull, volume operation or exec returns an error saying what failed and what to
  check, where it used to panic. A failed pull no longer leaves a stored config behind.
- `rooz update` pulls the image before removing the old containers, so a failed pull leaves the
  workspace as it was (an image that only comes from `bases` is still pulled afterwards).
- `rooz remote` log lines color each host name consistently.

## [0.163.0] - 2026-10-16

### Added

- `install` steps can be objects: `run`, `skip_if` and `root`. `skip_if` is a shell check that runs
  first; exit 0 skips the step, so a tool that is already installed (typically from a cache volume)
  is not installed again. `run` and `skip_if` are templated.

### Changed

- **Breaking:** install steps run as the workspace user, not root (sidecars: as their `uid`). The
  user and its volumes are set up before the first step. Steps that need root, such as `apt-get` or
  `apk add`, must say so with `root: true`. A configuration the operator did not author (unlisted
  `--config git:...` or `bases` layer) is refused if it asks for `root: true`.

## [0.162.0] - 2026-10-15

### Added

- `file: <path>` on a data value (`data`, inline `mounts`, sidecar `mounts`) takes its content from
  a file next to the config that declares it, for scripts and dotfiles that are clumsy inline. Works
  for local configs, bases and configs from git (read from the checkout). The path stays below the
  declaring config's directory; symlinks, files over 64 KiB and non-UTF-8 files are refused. The
  content is not templated, and the stored config keeps the reference.

## [0.161.0] - 2026-10-15

### Added

- `rooz remote` serves many hosts at once. Hosts are defined in `~/.config/rooz/remotes.yaml`
  (`rooz remote add|rm|ls`); `rooz remote [names...]` forwards each host's engine socket to
  `$XDG_RUNTIME_DIR/rooz/<host>.sock` (`~/.rooz/<host>.sock` without it) and its published ports to
  localhost. A host that is down retries with backoff without affecting the others.
- Global `-H/--host <name>` (env `ROOZ_HOST`) runs any command against a configured host.
  `rooz list --all-hosts` lists every connected host. `rooz code` attaches VsCode through the
  selected host.
- `rooz system prune` and `rooz remove --all` print the engine they target.

### Changed

- **Breaking**: `rooz remote` no longer reads `ROOZ_REMOTE_SSH_URL` / `DOCKER_HOST`, nor forwards to
  `$DOCKER_HOST`; define the host with `rooz remote add` instead.
- A host may set `socket` to skip the `$DOCKER_HOST` lookup over ssh; an unset one is now an error
  instead of a panic.
- A local port wanted by two hosts is forwarded for the first one and skipped (with a single
  warning) for the other.
- Errors are printed readably: the first sentence as a headline, the rest wrapped (to `$COLUMNS`,
  at most 100) below it - instead of one long quoted `Error: "..."` line. This covers the
  `ROOZ_ALLOW_PRIVILEGED`, `ROOZ_ALLOW_SECRETS`, `ROOZ_ALLOW_VPN` and `ROOZ_TRUSTED_CONFIGS`
  refusals, among others. The exit code is still 1.
- Tunnels are dropped and reopened after an SSH reconnect, and the forwarded socket is removed on
  exit.

### Fixed

- `git pull`/`git fetch` over ssh inside a workspace no longer print `hostfile_replace_entries:
  mkstemp: Read-only file system`. The ssh key volume is mounted read-only in workspaces, and ssh
  kept trying to rewrite `known_hosts` there. The volume now carries an ssh client config with
  `UpdateHostKeys no`, written by `rooz system init` (also on an already initialized engine, so
  re-running it fixes existing setups) and on clone.

## [0.160.0] - 2026-10-12

### Added

- `vpn: true` on a sidecar grants `NET_ADMIN`, `/dev/net/tun` and
  `net.ipv4.conf.all.src_valid_mark=1` - enough for `wg-quick` (kernel or `wireguard-go`) and
  other in-container VPNs, without `privileged`. Requires operator consent naming the sidecars,
  `ROOZ_ALLOW_VPN=<names>` (or `=true`), separate from `ROOZ_ALLOW_PRIVILEGED`; a `vpn` sidecar
  must run as uid 0.
- `ROOZ_TRUSTED_CONFIGS` lists configuration sources that are the operator's own: repository URLs
  (matched exactly as passed to `--config`, never over `http://`/`git://`) and local directories
  (every file, bases included, must resolve inside one). A trusted source may expand secrets
  (still subject to `ROOZ_ALLOW_SECRETS`) and declare caches - including through `bases` - which
  previously only a local `--config` file without bases could.

### Removed

- **Breaking**: rooz no longer reads a `.rooz.yaml` from the root of a repository it clones
  (`rooz new -g <url>`). Applying a repository's configuration is now always explicit:
  `rooz new ws -g <url> --config <url>//.rooz.yaml`. Same trust rules as before - it is a
  `--config git:...` source, authored by the repository. Existing workspaces created from an
  in-repo config keep updating: `rooz update` re-fetches it as a `--config git:...` source.

### Fixed

- A `rooz new` refused for a missing `ROOZ_ALLOW_PRIVILEGED` (or `ROOZ_ALLOW_VPN`) consent no longer
  leaves the workspace's config volume behind, so re-running the same command with consent works
  instead of failing with "Workspace already exists". A refused `rooz update` keeps the previously
  stored config.
- `rooz update` keeps the workspace's repository (`-g`) when the config came from anywhere other
  than the repository's own `.rooz.yaml`. Previously it forgot it: the work container started in
  `/work` instead of the clone, and `--purge` removed the clone without re-cloning.

## [0.159.0] - 2026-09-24

### Security

- Sidecar runtime images are reused only by the image id rooz recorded when it committed them
  (kept on the sidecar container as `dev.rooz.runtime-image`), never by the predictable
  `localhost/rooz/<workspace>/<sidecar>` name. An image tagged under that name by somebody else
  is ignored and the install stage runs again.
- An age identity found in the engine's `rooz_sys-config` volume is no longer adopted as this
  machine's identity. It is taken out of the volume and kept at `~/.config/rooz/age.engine.bak`
  for the operator to recognise and install with
  `rooz system init --force --age-identity "$(cat ...)"`, because anyone with access to that
  engine can write a key there.
- Configuration that the operator did not author can no longer abort rooz mid-creation: port
  mappings, `env` keys, `mounts` naming an undefined `data:` entry and non-YAML `bases` paths are
  refused with a message instead of panicking, before any volume or container is created.
- `env` keys must be identifier-shaped, and the `ROOZ_META_*` namespace rooz injects itself is
  reserved - a config can no longer forge the metadata a workspace reads back.
- Engine-supplied labels are treated as untrusted input: `rooz list` skips volumes that carry
  rooz's role label without a workspace label (previously a panic for every operator on that
  engine), and a duplicated label set reports an error instead of aborting.
- Every GitHub Action is pinned to a commit SHA, with Dependabot raising upgrades as reviewable
  pull requests.

### Changed

- **Breaking**: expanding `secrets` now needs explicit consent - `ROOZ_ALLOW_SECRETS=<names>`
  (comma-separated, safe to leave in a shell profile and also covers `rooz update`),
  `ROOZ_ALLOW_SECRETS=true`, or `--allow-secrets true`. A local `--config` path is not proof that
  the operator wrote the file: repositories ship config files and README instructions to pass
  them. Workspaces without `secrets:` are unaffected.
- `FileFormat::from_path`, `RoozCfg::parse_ports`, `VolumeApi::create_volume_specs` and
  `VolumeApi::mounts_with_sources` return `Result` instead of panicking.

### Documentation

- README states the trust model: the engine is assumed to be the operator's own, and an engine
  shared with people who do not trust each other is out of scope.
