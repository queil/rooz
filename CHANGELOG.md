# Changelog

Notable changes per release. The release workflow refuses to publish a tag whose version has
no section here, and the section becomes the release notes - so write it before bumping
`Cargo.toml`, then rename `Unreleased` to the version you are tagging.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Releases before 0.159.0 predate this
file - see the git history.

## [0.164.0] - 2026-10-16

### Added

- `rooz new --dry-run` and `rooz update --dry-run` (also with `--purge`) print what would be removed
  and created - image pull, volumes, caches, networks, sidecars, the work container and install steps
  - and change nothing. The configuration is resolved and gated like a real run, so a plan fails with
  the error a run would. Env values, scripts and secrets are not printed.

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
