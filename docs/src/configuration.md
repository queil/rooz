# Configuration

ℹ️ Rooz uses YAML for configuration.

Settings come from env vars, command-line parameters, and a config file (`--config` on `rooz new`, most options - [examples](https://github.com/queil/rooz/tree/main/examples)).

* `--config` takes a local path or a git file, `<repo-url>//<path-in-repo>`, e.g. `git@github.com:my/configs//path/in/repo/config.rooz.yaml`. The repo is cloned with rooz's SSH key, so only SSH URLs work: scp-like `git@host:repo` or `ssh://git@host[:port]/repo` (also `git+ssh://`, `ssh+git://`). Other schemes are refused.
* Rooz does not read config from cloned repositories. Pass it explicitly: `rooz new ws -g <url> --config <url>//.rooz.yaml`

## What a repository's configuration may set

Repository config (a `--config <url>//<path>` source, e.g. its `.rooz.yaml`) may set fields scoped to the workspace: `bases`, `git_ssh_url`, `image`, `shell`, `command`, `args`, `env`, `install`, `ports`, `data`, `mounts`, `sidecars`, `extra_repos`, `vars`, `user`, `init`.

Refused from repository config:

| field | reason | to allow |
| --- | --- | --- |
| `secrets` | plaintext lands in a repo-controlled container | list the source in `ROOZ_TRUSTED_CONFIGS` ([Trusted config sources](secrets.md#trusted-config-sources)), or use a local `--config` file |
| `caches` | shared with your other workspaces | same paths in `ROOZ_CACHES` (or `--caches`, which covers `rooz new` only, not `rooz update`), or source in `ROOZ_TRUSTED_CONFIGS` |
| `install` step `root: true` | runs as root in the workspace | source in `ROOZ_TRUSTED_CONFIGS`, or use a local `--config` file ([Install steps](#install-steps)) |
| `privileged` | full host access | `ROOZ_ALLOW_PRIVILEGED=<names>` (required for *any* config file) |
| sidecar `vpn` | kernel netfilter access as root on a rootful engine | `ROOZ_ALLOW_VPN=<names>` (required for *any* config file) |

## Clone transports

Clone URLs (root repo, `extra_repos`) accept `ssh://`, `git+ssh://`, `ssh+git://`, scp-like `user@host:path`, `https://`, `http://`, `git://` or `file://`. Anything else is refused, including `ftp://` and remote helpers (`name::address`, e.g. `ext::`). Remote `--config` sources take SSH URLs only (see above).

`http://` and `git://` are unauthenticated (a network attacker controls the clone); rooz warns. Set `ROOZ_REQUIRE_SECURE_TRANSPORT=true` to refuse them.

## Images

ℹ️ Default image: `docker.io/chainguard/git:latest-dev`

Set the image via:
* `ROOZ_IMAGE` env var
* `--image` parameter
* `image` in a config file (`--config`):

  ```yaml
  image: ghcr.io/queil/image:dotnet
  shell: [bash]
  ```

## User

Rooz runs as uid `1000` (override with `--uid` on `rooz new`); the image must have this user, named `rooz_user` by default. Override the name with `ROOZ_USER` or `--user`: letters, digits, `_`, `-`, `.`, not starting with `-` or `.`. Home is `/home/<user>`: the SSH key volume mounts there and `~/` in mount targets expands to it.

## Shell

Default: `sh` (`bash` for `rooz tmp`). Override via:

* `ROOZ_SHELL` env var (on `rooz new`, `rooz update`, `rooz tmp`)
* `--shell` parameter (on `rooz enter`, `rooz tmp`)
* `shell` in a config file (a list, e.g. `[bash]`)

## Caching

Path-keyed shared caches:

```yaml
caches: 
- ~/.nuget
```

Workspaces with the same cache path share one volume at that path. Also set via `ROOZ_CACHES` (comma-separated, merged with config paths) or `--caches`.

⚠️ Cache contents (cargo sources, linker config, build scripts) execute in every workspace mounting that path. Cache paths from repository config (a `--config <url>//<path>` source not in `ROOZ_TRUSTED_CONFIGS`) are refused unless you also list them in `--caches` (on `rooz new`) or `ROOZ_CACHES`. `rooz update` re-reads repository config each run, so only `ROOZ_CACHES` covers it. Local `--config` files are unaffected.

Cache volumes are scoped per operator (default: rooz process uid), so operators on a shared engine (remote `DOCKER_HOST`, multi-user socket) get separate volumes - toolchain caches hold executable config (`~/.cargo/config.toml`, `~/.m2/settings.xml`, ...). If uids collide across machines on one engine, set a distinct `ROOZ_CACHE_SCOPE` per operator. The scope is not access control: anyone who can create workspaces on the engine can use any scope.

## Dry run

`rooz new <name> --dry-run` and `rooz update <name> --dry-run` print the plan and change nothing:

```
$ rooz new demo --config demo.rooz.yaml --dry-run
pull     image     docker.io/alpine:latest
create   volume    rooz_demo_work
reuse    cache     rooz_cache_uid-1000_---nuget  ~/.nuget
create   network   demo-egress
create   network   demo-net-db
create   sidecar   db  image=docker.io/postgres:17 install: 00-x[root]
create   work      demo  image=docker.io/alpine:latest user=rooz_user uid=1000 env=PGHOST
run      install   00-tools  root,skip_if

Dry run: nothing was changed.
```

* Config is resolved and gated as in a real run (bases, templating, secrets, consent, trust); errors match.
* `update` also lists containers to remove (and volumes, with `--purge`).
* Env values, install scripts and secrets are not printed - only env names and step names/flags.
* Queries the engine, creates nothing (no volumes, networks, containers, clones). A `--config <url>//<path>` source is fetched via a temporary helper container.
* `skip_if` runs in the container, so a listed step may still be skipped.

## Install steps

`install` runs shell steps in the work container on workspace creation. A step is a string or an object:

```yaml
vars:
  dotnet: 11.0.100-rc.2.26504.105

install:
  00-tools:
    run: apk add jq
    root: true
  10-dotnet:
    skip_if: dotnet --list-sdks | grep -q '^{{ dotnet }}'
    run: ./install-dotnet.sh {{ dotnet }}
```

* Steps run in key order as the **workspace user**; `root: true` runs as root. Sidecar steps run as the sidecar's `uid` (default root).
* `skip_if` runs first as the same user; exit 0 skips the step. Combine with a [cache](#caching) to skip repeat downloads. `run` and `skip_if` are templated.
* `skip_if` sees the container env: put cached tools on `PATH` (via `env`) or use absolute paths.
* `bases` layers add steps by key, replace whole steps, or remove with `null`.
* `root: true` is refused from untrusted config (unlisted `--config <url>//<path>` source or `bases` layer). Add the source to `ROOZ_TRUSTED_CONFIGS` or use a local config.

## Sidecars

A sidecar with `install` steps is built once: rooz runs the steps, commits `localhost/rooz/<workspace>/<sidecar>:latest`, and starts the sidecar from it.

* The sidecar uses the committed image **id**, not the name (tags and labels are forgeable). The id is recorded on the sidecar container (`dev.rooz.runtime-image`).
* Later runs skip install only if that id still exists and matches; otherwise install reruns and the tag is rebuilt.
* Removing a workspace removes its built images.

## Privileged containers

`privileged: true` in any config file (local or git, including sidecars) aborts workspace creation and names the containers. To allow:

* `ROOZ_ALLOW_PRIVILEGED=<names>` - comma-separated container names, e.g. `ROOZ_ALLOW_PRIVILEGED=dkr`. Unnamed containers are still refused.
* `ROOZ_ALLOW_PRIVILEGED=true` - allows any. Use for CI only.
* `--privileged true` - consents *and* makes the work container privileged regardless of config.

Prefer names over `true`, e.g. name only the docker-in-docker sidecar, not the workspace using it via `DOCKER_HOST`. Names are safe to keep in your shell profile; a standing `=true` also allows privileged containers added by repository config.

⚠️ Only for trusted configs. For repositories you do not control, prefer rootless Podman (confines the grant to your user namespace).

## VPN sidecars

`vpn: true` on a sidecar grants what WireGuard (`wg-quick`, kernel or `wireguard-go`), OpenVPN etc. need: `NET_ADMIN`, `/dev/net/tun` and `net.ipv4.conf.all.src_valid_mark=1`, scoped to the sidecar's network namespace (no host devices or network, unlike `privileged`).

It exposes kernel netfilter/netlink, a container-escape surface on rootful engines, so it requires `ROOZ_ALLOW_VPN=<names>` (or `=true` for any). Independent of `ROOZ_ALLOW_PRIVILEGED`.

```yaml
sidecars:
  vpn:
    image: docker.io/linuxserver/wireguard:latest
    vpn: true
    egress: true     # to reach the WireGuard endpoint
    init: false      # the image's s6-overlay must run as pid 1
    mounts:
      /config/wg_confs/wg0.conf:
        content: |
          [Interface]
          ...
```

* Must run as uid 0 (default); `vpn` with another `uid` is refused (`NET_ADMIN` is ineffective for non-root).
* Kernel WireGuard needs the `wireguard` module loaded on the engine host (containers cannot load it); otherwise `wg-quick` falls back to `wireguard-go`.
* Full tunnels (`AllowedIPs = 0.0.0.0/0`) work; the work container still reaches the sidecar over its pair network.
* Omit `DNS =`: `resolvconf` conflicts with the engine-managed `/etc/resolv.conf`.
* The tunnel is the sidecar's only. Expose it via a proxy in the sidecar, e.g. `HTTP_PROXY=http://vpn:3128` for the work container (or `peers:`).

## Files next to the config

`file:` loads content from a file (like a Kustomize config map generator). Works wherever `content:` does: `data`, inline `mounts`, sidecar `mounts`.

```yaml
data:
  hello.sh:
    file: files/hello.sh
    executable: true
mounts:
  ~/bin/hello.sh: hello.sh
  ~/.config/starship.toml:
    file: files/starship.toml
```

* Path is relative to, and below, the declaring config's directory (a base's to the base). No absolute paths, no `..`.
* Git configs read from the same checkout.
* Symlinks are refused anywhere on the path.
* Max 64 KiB, UTF-8 text. For larger or binary data use a volume or `generate:`.
* Not templated; use `content:` for `{{ vars }}`.
* The stored config keeps the reference, not the content.

## Port mappings

Work container ports - config file only. Format `[<host port>:]<container port>`, as in Docker, bound to `127.0.0.1`:

```yaml
ports:
- "8080:80"
- "8022:22"
```
