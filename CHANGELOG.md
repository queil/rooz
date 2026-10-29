# Changelog

Notable changes per release. The release workflow refuses to publish a tag whose version has
no section here, and the section becomes the release notes - so write it before bumping
`Cargo.toml`, then rename `Unreleased` to the version you are tagging.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Releases before 0.159.0 predate this
file - see the git history.

## [0.175.1] - 2026-10-28

### Fixed

- An egress `inject` host that an `allow` wildcard covers (`nuget.pkg.github.com` under
  `*.github.com`) is intercepted instead of stopping the proxy at startup; only the same name in both
  lists is refused, and rooz refuses it before creating anything, naming the sidecar and the host.
- Across config layers, a host a later layer injects leaves the lower layers' `allow`, and one it
  allows leaves their `inject`, instead of ending up in both.

## [0.175.0] - 2026-10-28

### Added

- `rooz egress log`: a workspace's egress proxy decisions, counted by host with refusals first and
  colored by decision; `--denied` for what was refused, `--suggest` for the refused hosts as a policy
  to paste, `-f` to follow. The proxy logs each decision as a line of JSON.
- `learn: true` in an egress policy lets through what `allow` and `ports` would refuse, logged as
  `learn`, to find out what a sidecar needs.

### Changed

- The egress proxy logs each decision as a line of JSON, so `rooz logs <ws> -c <sidecar>-egress` shows
  JSON; `rooz egress log` (`-f` to follow) is the readable view. A proxy still running an older joey
  logs text until `rooz update` recreates it.
- A sidecar's egress policies from several config layers combine instead of the last replacing the
  rest: `allow` and `ports` are unions, `inject` goes by host (a later layer's entry for a host
  replaces the earlier one). `true` or `false` still replaces a policy.
- rooz's helper image, also the default workspace image, is joey `0.174.0-tools` (was `0.171.0-tools`),
  pinned by digest.

### Fixed

- A container that exits right after starting is reported as such - its name, its exit code and
  where to look - instead of the engine's message about exec sessions on a container not running.

## [0.174.0] - 2026-10-26

### Added

- `dir:` mounts: every file below a directory next to the config, each at the same relative path
  below the mount path, e.g. `~/: {dir: home}` for dotfiles. Files go in one by one, so the rest of
  the mount path stays; a path mounted on its own keeps that mount. The `file:` rules apply to each
  file, with at most 256 files and 1 MiB per directory.

### Changed

- A mount path with a `.` or `..` segment (`~/../etc/x`) is refused: it names where a mount lands,
  and rooz acts as root on the directories above it.

### Fixed

- A file or volume mounted below a directory missing from the image (`~/.config/app.conf` in an image
  without `~/.config`) no longer leaves that directory owned by root: in the work container's home,
  rooz hands it to the workspace user.
- A sidecar behind an egress policy reaches the sidecars that name it in their `peers` directly, as
  it does its own peers: they are in its `NO_PROXY` too. A registry mirror peering with a proxied
  `dkr` was refused by the proxy (plain HTTP).

## [0.173.1] - 2026-10-26

### Added

- The egress proxy's log lines end with the client's User-Agent (`[ua: ...]`, `-` when it sends
  none), to tell which tool in a sidecar made a request.

### Changed

- Refusals of configuration not wholly yours (secrets, root install steps, `extra_repos`, shared
  caches) are now two lines: what was refused, then how to allow it with a docs link.

### Fixed

- An `egress:` that parses as neither `true`/`false` nor a policy says what is wrong with it (an
  unknown or duplicate field, a wrong type) instead of only that it matched no variant.
- The egress example in the sidecar docs referenced a secret as `{{ secrets.<name> }}`; it is
  `{{ <name> }}`, like any secret.

## [0.173.0] - 2026-10-25

### Added

- `joey egress-proxy`, the first work for rooz's helper binary: an HTTP `CONNECT` proxy that decides
  on host and port before dialling, holds an allowed host's TLS to the server name it was opened
  for, and intercepts only the hosts with header rules (for a token the client must not hold).
  See `joey/README.md`.
- An egress policy for sidecars: `egress: {allow: [...], inject: [...]}` puts a sidecar behind a proxy
  of its own (`<name>-egress`, running `joey egress-proxy`), its only way out. Allowed hosts are
  reached with their own TLS; `inject` hosts get header rules applied by the proxy, so a token in
  `secrets` never reaches the sidecar. rooz generates the workspace's CA for it, mounts the
  certificate in the sidecar and sets `HTTP(S)_PROXY`, `NO_PROXY` and `NODE_EXTRA_CA_CERTS`. The proxy
  URL carries a credential generated per proxy, and the proxy serves no client without it.
  `egress: true` and `false` are unchanged. The proxy runs `ghcr.io/queil/joey` of rooz's own version,
  published by the same release; `ROOZ_EGRESS_IMAGE` runs another image, for trying a joey build.

### Fixed

- On podman, a short-lived rooz container (`system init`, volume population) could fail with
  `no exec session ... found`: podman removes a command's exec session with its container, and rooz
  read the command's exit code from the session after the container was gone. It is read from the
  container now.
- On podman, `rooz move --to` handed every file in the moved volumes to root: podman's archive
  download reports all files as owned by root. The volume is now read with `tar` inside a container,
  which keeps owners and modes.
- On podman, `~` in a workspace was the working directory, not the user's home: podman fixes `HOME`
  when a container is created, before rooz adds its user, so `~/...` mounts were missed. The work
  container's `HOME` is set explicitly now (unless `env` sets it), and the user rooz adds replaces
  any account podman made for the same uid.

## [0.172.0] - 2026-10-25

### Changed

- rooz runs its own containers (clones, volume population, key generation, `cache ls`) from joey's
  tools image, `ghcr.io/queil/joey:0.171.0-tools`, pinned by digest, instead of
  `docker.io/chainguard/git:latest-dev`. It is also the default workspace image and the default
  image of `generate:` scripts, so a config without `image:` now gets Alpine instead of Wolfi; set
  `image: docker.io/chainguard/git:latest-dev` to keep the old one. An image pinned by digest is not
  pulled again by `--pull-image` or `rooz update`: its content cannot change.
- `rooz cache ls` measures only the cache volumes it lists, with `du` in one short-lived container,
  instead of asking the engine for its disk usage report, which sizes every image, container and
  volume and took long on a busy engine.
- A clone no longer warns `could not create the container user` when the git container's root
  filesystem is read-only (podman): the clone does not need the account.

## [0.171.0] - 2026-10-24

### Added

- `joey`, rooz's helper, as a crate of its own in `joey/` that carries rooz's version. Each release
  publishes it as `ghcr.io/queil/joey:<version>` (the static binary alone) and
  `ghcr.io/queil/joey:<version>-tools` (with `sh`, `git` and `openssh-client`), for amd64 and arm64.
  rooz does not use either yet.

### Changed

- Clones are full instead of `--filter=blob:none`: `git log`, `git show` and checkouts no longer
  need the remote (a partial clone fetches missing blobs on demand, which fails in a workspace or
  sidecar with no ssh or no route out). Each repository is mirrored bare in a cache volume that is
  refreshed on every clone, and the clone borrows its objects and then drops the link, so cloning the
  same repository again is quick. A mirror that cannot be updated falls back to cloning from the
  remote. Config clones (`--config <url>`), shallow clones and `file://` URLs do not use it. Git never
  runs against the remote from inside a mirror, ignores a mirror another user owns, and keeps its
  mirrors readable and writable by their owner only.
- When `rooz new` fails after the workspace's config was stored (an install step failed, say), the
  error ends with `run 'rooz update <name>' to resume from the last completed install step`,
  naming the host with `-H`.

### Fixed

- A clone that failed (no terminal, an unreachable host), or a `--config <git url>` whose file
  could not be read from the clone (missing, invalid, naming a missing data file or base), left the
  clone container running with the SSH key volume mounted, and `rooz rm` did not remove it. The
  container is removed once rooz is done with it, whatever the outcome.
- A git clone that failed went unnoticed: a failed `--config <git url>` clone was reported as
  `Config file ... not found or empty`, and a failed `extra_repos` clone was skipped. Any failed
  clone now fails the command with `git clone failed (exit code N)` after git's own message.
- `rooz update --rebuild` runs every install step, the work container's and the sidecars', whatever
  its `skip_if` says. A `skip_if` that looks at a volume (`test -x ~/.cargo/bin/rustc`) was true
  while the rebuilt image lacked the other half of the install (`~/.rustup`), so the step was
  skipped and the workspace stayed broken.

## [0.170.0] - 2026-10-22

### Added

- Install progress survives a failed step. After each step that runs, the container is saved as a
  checkpoint image (`localhost/rooz/<workspace>/<container>-checkpoint`), for the work container
  and for sidecars. The next attempt starts from the latest checkpoint that still matches the base
  image, the user, the environment and the steps up to it, and runs only the steps after it:
  a failed `emacs` step no longer throws away what the earlier ones downloaded. Resumed steps show
  as `↺`. Checkpoints are removed once the install succeeds, on `rooz rm`, and by a normal
  `rooz update`; `rooz update` of a workspace whose creation failed keeps them (`--rebuild` and
  `--purge` start over).

### Fixed

- `rooz update` finishes a workspace whose creation failed before its runtime config was stored,
  instead of failing with `missing field extra_repos`; `rooz enter` names the unfinished creation.

## [0.169.6] - 2026-10-22

### Changed

- `rooz new` populates the volumes of different sidecars concurrently. Populations that target the
  same volume (the shared inline volume) stay sequential, as they rewrite the same tree.
  The helper image they run in is ensured once beforehand, so concurrent runs don't each pull it.

## [0.169.5] - 2026-10-21

### Changed

- On a terminal, an image pull is one live line with the whole image's progress
  (`⬇ grafana/grafana ████░░░░ 25% 50 MB / 200 MB`) instead of a static "Pulling" line, so
  several pulls side by side each show how far along they are. A finished pull leaves
  `✓ Pulled <image> · <size>`.
- The privileged and vpn consent refusals are two lines: what was refused, the value to set, and a
  link to the docs. The explanation (pinning, fingerprints, `=true`) lives on the configuration
  page.

## [0.169.4] - 2026-10-20

### Fixed

- With `-H`, the hint printed after `rooz new` (and the "already exists" one) names the host:
  `rooz -H prod enter ws`, not `rooz enter ws`, which would look on the default engine.

## [0.169.3] - 2026-10-20

### Fixed

- `rooz exec` and `rooz enter` work again in images without a writable `/etc/passwd`
  (distroless, scratch-based). 0.169.2 made a failed container-user creation fatal; it is a
  warning that names the cause again.

## [0.169.2] - 2026-10-19

### Fixed

- A clone directory is named after the repository: only a trailing `.git` is dropped
  (`my.github.io` was cloned as `myhub.io`), and `git@host:repo.git` no longer lands in a
  directory with `@` and `:` in its name.
- `url.<base>.insteadOf` rewrites use the longest matching alias and every `insteadOf` value in a
  section, as git does.
- `rooz exec` exits non-zero when the engine reports no exit code or a negative one. Before, both
  looked like success.
- A failed `rooz move` restarts the source workspace it had stopped, and no longer leaves the
  destination's volumes behind when the copy container is still being removed. Removing a volume
  waits briefly for a container that is going away.
- `rooz cache rm` given the same cache twice removes it once instead of failing midway.
- `rooz enter`, `start`, `stop` and `restart` no longer panic on a container missing from the
  stored config or with an unexpected name list.
- Image references with a registry port (`images:5000/alpine:3`) or a digest (`img@sha256:...`)
  are pulled as written; they were split on every `:`.
- A command run in a container fails when the engine does not report its exit code, instead of
  counting as a success. Output split mid-character across chunks no longer fails the command.
- A container whose extra network cannot be connected is removed instead of being adopted
  half-networked by the next run. `kill` no longer spins and panics on podman.
- Creating the container user matches the uid exactly: uid 100 no longer deletes the `passwd`
  entry of uid 1000.
- A config that does not parse is an error for `new`, `update` and `config edit`; before, a local
  one was ignored with a warning (creating the workspace with defaults) and a git one panicked.
  `config edit` without a terminal to retry in stops instead of reopening the editor forever.
- Non-adjacent duplicate `caches` are dropped. A secret that is part of another one no longer
  leaves the rest of it readable in the stored runtime config.
- `rooz tmp` removes its workspace when the shell cannot be entered. `rooz enter` waits between
  reconnect attempts and no longer panics on a container missing from the stored config.
- `rooz remote` keeps the leading `/` of a `unix:///...` `DOCKER_HOST`; the forward pointed at a
  relative path.
- Stack detection skips a project file it cannot read instead of failing.
- `rooz update` resolves and gates the new configuration before removing anything: a refused
  consent or an invalid value no longer leaves the workspace (or, with `--purge`, its volumes)
  gone. A workspace made from flags alone keeps its image, user, caches, privileged flag and
  `--uid` across `update`; before, it was rebuilt with the defaults.
- `--uid` is stored with the workspace, so `move`, `enter --chown` and `plan` see it.
- An undefined template variable (`{{ tga }}`) is an error naming it instead of an empty string,
  and `vars` are rendered once, in order, so a forward reference is an error too.
- `extra_repos` in a remote `--config <url>//<path>` source (or a config merging an untrusted
  `bases` layer) is refused unless the source is in `ROOZ_TRUSTED_CONFIGS`: the clone uses your ssh key.
- An interrupted `system init` that left an empty ssh-key volume is completed by running it
  again.
- `rooz remote`: one failed Docker API call no longer closes every forwarded port, and a service
  that accepts the tunnel probe but never answers cannot stall the poll loop.
- `update --purge --dry-run` no longer lists the config volume, which a purge keeps.
- The terminal is restored when an `enter` session ends with an error, a failed container-user
  creation is reported instead of ignored, and a failed `chown` is warned about.
- `rooz move --to` hints no longer print a stray `\`.

## [0.169.1] - 2026-10-19

### Changed

- A finished container's install summary lists its steps: `✓` ran (with its time), `○` skipped by
  `skip_if`; a sidecar whose image is reused shows `↺`.

### Added

- `rooz update --rebuild` builds sidecar images again even when their install steps are unchanged,
  without touching volumes. Before, a broken image was reused until a step changed or `--purge`.

### Fixed

- When one container's install failed, the others were cut short, and a sidecar's half-built
  install container stayed behind under the sidecar's name: base image, no env, none of the steps'
  work. Every container's install now runs to the end, all failures are reported, and a sidecar
  install container is always removed.

## [0.169.0] - 2026-10-19

### Changed

- **Breaking:** privileged and vpn consent is pinned to the container's definition:
  `ROOZ_ALLOW_PRIVILEGED=dkr@<fingerprint>` instead of `=dkr`. A bare name also covered any other
  config declaring a `dkr`, e.g. an untrusted `bases` layer with its own image, which then ran as host
  root. The refusal prints the value to set; it changes with the container's image, command, env,
  mounts or install steps. `=true` and `--privileged true` work as before.
- **Breaking:** sidecar containers and workspace networks are named with `_`: `<workspace>_<sidecar>`,
  `<workspace>_egress`, `<workspace>_net_<sidecar>`, `<workspace>_peer_<a>_<b>`. With `-`, workspace
  `a` with sidecar `x` and workspace `a-x` shared a container name, and peer pairs `(a, b-c)` and
  `(a-b, c)` a network. The first `rooz update` after upgrading moves a workspace to the new names;
  until then it keeps working under the old ones. `ROOZ_META_CONTAINER_NAME` in sidecars changes
  accordingly.
- **Breaking:** sidecar names are lowercase letters, digits and `-`, starting with a letter or digit;
  `work` is reserved.
- **Breaking:** data keys that would share a volume (`a.b` and `a-b`), a data key named `inline`, and
  keys not starting with a letter or digit are refused, as are two inline mounts of one container whose
  paths would share a file.
- A sidecar's inline mount files are kept apart from the work container's: the same path inline in
  both is two files. A sidecar's `once` inline file is written afresh once after upgrading.
- Sidecar images build, and their install steps run, alongside the work container's install instead
  of one after another. On a terminal each container gets a live pane with its current step and last
  output lines; elsewhere, or with `ROOZ_PROGRESS=plain`, output lines are tagged with their container
  and step.
- Install progress says why a step was skipped (`skipped: skip_if succeeded`), and `rooz update` prints a
  line when it reuses a sidecar's built image instead of running its steps.

### Fixed

- A container or network found under a workspace's name is reused only if its labels say it belongs to
  that workspace (and container); anything else is refused instead of adopted or replaced.
- An invalid sidecar name panicked halfway through `rooz new`; container creation errors are now
  reported as errors.
- Caches from a `bases` layer not in `ROOZ_TRUSTED_CONFIGS` were shared without consent when the
  top-level config was a local file. They now need `ROOZ_CACHES`, like a repository's.
- Secret values were rendered as templates: a `{{` in a secret corrupted it, let the result escape
  masking, or printed the plaintext in a template error. They are now inserted verbatim.
- A key in both `vars` and `secrets` panicked; it is now an error.
- Relative directories in `ROOZ_TRUSTED_CONFIGS` resolved against the current directory, which inside a
  cloned repository could trust its own files. They are now ignored with a warning.

## [0.168.3] - 2026-10-19

### Fixed

- `rooz update` rebuilt every sidecar with `install` steps from scratch, so the steps ran again and
  `skip_if` could never skip them. It now reuses the built image while the base image, `uid`,
  `command`, `args` and steps are unchanged.
- `rooz update` did not pull sidecar base images when the workspace's image was set in the config,
  so a floating tag such as `alpine:3` never moved. It now pulls them unless `--no-pull` is given.

## [0.168.2] - 2026-10-19

### Added

- `once: true` on a `data` file (`content:`, `file:` or `generate:`): written only when nothing is at
  its path, then left to the containers. For history files, generated keys, or a file a sidecar fills
  in and the work container reads.
- A VS Code page in the docs: `rooz code`, keeping extensions across rebuilds, per-workspace editor
  setup and remote hosts. The landing page links to it.
- A "Data and mounts" section in the configuration docs.

### Changed

- `rooz update` writes every non-`once` data file from the config, overwriting changes made in the
  container, and re-runs `generate:` scripts. Mark files that containers own `once: true`.
- Moving a workspace keeps edits made in its sidecars' data files; they used to be rewritten from the
  config on every run. Only `rooz update` rewrites files.
- Install progress reads `[install:{container}:{step}]`, with the container in a stable colour.
  Sidecars show their short name (`claude`, not `rooz-dev-claude`). The step line is printed by rooz,
  so it is uncoloured when output isn't a terminal.

### Fixed

- `rooz update` ignored edited data files in the work container, and failed on a newly added one,
  leaving the workspace without a container.
- `rooz new` without `rooz system init` refuses with a hint. It used to make the engine create an empty,
  unlabelled `rooz_ssh-key` volume, which `rooz system init` then refused to reuse.
- `tests/interactive.rs` builds on macOS.

## [0.168.1] - 2026-10-17

### Added

- The docs site shows the release version in the top bar.

### Fixed

- `rooz enter` follows terminal resizes; the session used to keep the size it started with.

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
