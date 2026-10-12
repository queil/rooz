# Changelog

Notable changes per release. The release workflow refuses to publish a tag whose version has
no section here, and the section becomes the release notes - so write it before bumping
`Cargo.toml`, then rename `Unreleased` to the version you are tagging.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Releases before 0.159.0 predate this
file - see the git history.

## Unreleased

### Removed

- **Breaking**: rooz no longer reads a `.rooz.yaml` from the root of a repository it clones
  (`rooz new -g <url>`). Applying a repository's configuration is now always explicit:
  `rooz new ws -g <url> --config <url>//.rooz.yaml`. Same trust rules as before - it is a
  `--config git:...` source, authored by the repository. Existing workspaces created from an
  in-repo config keep updating: `rooz update` re-fetches it as a `--config git:...` source.

### Fixed

- `rooz update` keeps the workspace's repository (`-g`) when the config came from anywhere other
  than the repository's own `.rooz.yaml`. Previously it forgot it: the work container started in
  `/work` instead of the clone, and `--purge` removed the clone without re-cloning.

## [0.160.0] - 2026-10-12

### Added

- `vpn: true` on a sidecar grants `NET_ADMIN`, `/dev/net/tun` and
  `net.ipv4.conf.all.src_valid_mark=1` - enough for `wg-quick` (kernel or `wireguard-go`) and
  other in-container VPNs, without `privileged`. Requires operator consent naming the sidecars,
  `ROOZ_ALLOW_VPN=<names>` (or `=true`), separate from `ROOZ_ALLOW_PRIVILEGED`; a `vpn` sidecar
  must run as uid 0.

### Fixed

- A `rooz new` refused for a missing `ROOZ_ALLOW_PRIVILEGED` (or `ROOZ_ALLOW_VPN`) consent no longer
  leaves the workspace's config volume behind, so re-running the same command with consent works
  instead of failing with "Workspace already exists". A refused `rooz update` keeps the previously
  stored config.

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
