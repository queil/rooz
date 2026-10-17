# Configuration

ℹ️ Rooz supports `yaml` as configuration formats.

Most of the settings can be configured via:

* environment variables
* a config file specified via `--config` (on `rooz new`)
  ℹ️ it can be a local file path or a remote git file like: `git@github.com/my/configs//path/in/repo/config.rooz.yaml`

  ℹ️ rooz does not look for configuration in the repositories it clones. To use a
  repository's own config file, pass it explicitly: `rooz new ws -g <url> --config <url>//.rooz.yaml`
* cmd-line parameters

The configuration file provides the most options: [example](examples/dotnet-nats.rooz.yaml)

## What a repository's configuration may set

Configuration authored by the repository being opened (a `--config git:...` source, such as its
`.rooz.yaml`) may set anything that only shapes the workspace container - `image`, `shell`, `command`,
`args`, `env`, `install`, `ports`, `data`, `mounts`, `sidecars`, `extra_repos`, `vars`, `user`, `init`. That
grants the repository nothing it does not already have: the workspace runs its code either way.

Fields that reach past the workspace - your host, your other workspaces, your secrets - are yours to set:

| field | why | how to allow it anyway |
| --- | --- | --- |
| `secrets` | plaintext would land in a container the repository controls | list the source in `ROOZ_TRUSTED_CONFIGS` if it is yours (see [Trusted config sources](secrets.md#trusted-config-sources)), or move the settings into your own local `--config` file |
| `caches` | a cache volume is shared with your other workspaces | list the same paths in `--caches` / `ROOZ_CACHES`, or list the source in `ROOZ_TRUSTED_CONFIGS` |
| `privileged` | full access to the host running the engine | `ROOZ_ALLOW_PRIVILEGED=<names>` (refused on *any* config file's say-so, including your own) |
| sidecar `vpn` | `NET_ADMIN` and `/dev/net/tun` - real root's reach into the kernel's netfilter code on a rootful engine | `ROOZ_ALLOW_VPN=<names>` (refused on *any* config file's say-so, including your own) |

## Clone transports

Clone URLs (the root repo, `extra_repos`, `--config git:...`) may use `ssh://`, `git+ssh://`,
the scp-like `user@host:path`, `https://`, `http://`, `git://` or `file://`. Anything else -
`ftp://`, invented schemes, and `name::address` remote helpers such as `ext::` (which hands git
a command to run) - is refused.

`http://` and `git://` authenticate nothing: whoever sits on the network path decides what the
clone contains, including any `--config` file rooz reads from it. Those clones
print a warning. Set `ROOZ_REQUIRE_SECURE_TRANSPORT=true` to refuse them outright.

## Images

ℹ️ the default image is `docker.io/chainguard/git:latest-dev`

There are a few ways of specifying images:
* via the `ROOZ_IMAGE` env variable
* via  the `--image` cmd-line parameter
* via `image` in a config file (`--config`):

  ```yaml
  image: ghcr.io/queil/image:dotnet
  shell: bash
  ```

## User

`rooz` runs as uid `1000` (always - it's hard-coded) so make sure it exists in your image
(with `rooz_user` as the name - it can be overridden via `ROOZ_USER` or `--user`)

The name must be a plain account name (letters, digits, `_`, `-`, `.`, not starting with `-` or
`.`): it becomes the workspace home `/home/<user>`, which is where the ssh key volume mounts and
what `~/` mount targets expand to.

## Shell

The default shell is `bash` but you can override it via:

* `ROOZ_SHELL` env var
* `--shell` cmd-line parameter (on `rooz enter`)
* in a config file via `shell`

## Caching

`rooz` supports basic path-keyed shared caches. It can be set in a config file like:

```yaml
caches: 
- ~/.nuget
```

All the workspaces specifying a cache path will share a container volume mounted at that path enabling cache reuse.
It also can be set globally via `ROOZ_CACHES` (comma-separated paths), or per workspace with `--caches`.
The global paths get combined with the config file's paths.

⚠️ A cache is shared with *every* workspace of yours mounting the same path, so whatever writes
into it (a cargo registry source, a linker config, a crate with a build script) later executes in those
workspaces. That makes a cache path an operator decision: if the configuration comes from the repository
being opened (a `--config git:...` source not listed in `ROOZ_TRUSTED_CONFIGS`), rooz refuses its cache paths
unless you list the same paths yourself with `--caches` (on `rooz new`) or `ROOZ_CACHES` - the latter also
covers `rooz update`, which re-reads a repository-authored config on every run. Config files you wrote
locally and pass with `--config` are unaffected.

ℹ️ Cache volumes are scoped to the operator, not just the path. The scope defaults to the
uid of the rooz process, so one person's workspaces share their caches while unrelated operators on a
*shared* engine (a remote `DOCKER_HOST`, a multi-user socket) get separate volumes. Toolchain caches hold
configuration that the toolchain executes (`~/.cargo/config.toml`, `~/.m2/settings.xml`, ...), so sharing
them across operators means sharing code execution.

If several machines share one remote engine and their local uids collide (both `1000`, say), set a
distinct `ROOZ_CACHE_SCOPE` per operator. Note that the scope is a collision boundary, not an
authorization boundary: anyone able to create workspaces on an engine can name any scope.

## Dry run

`rooz new <name> --dry-run` and `rooz update <name> --dry-run` report what the command would do and change
nothing:

```
$ rooz new demo --config demo.rooz.yaml --dry-run
pull     image     docker.io/alpine:latest
create   volume    rooz_demo_work
reuse    cache     rooz_cache_uid-1000_---nuget  ~/.nuget
create   network   demo-egress
create   sidecar   db  image=docker.io/postgres:17 install: 00-x[root]
create   work      demo  image=docker.io/alpine:latest user=rooz_user uid=1000 env=PGHOST
run      install   00-tools  root,skip_if

Dry run: nothing was changed.
```

* the configuration is resolved and gated exactly as for a real run (bases, templating, secrets, consent,
  trust), so a plan fails with the same error a run would.
* `update` also lists the containers (and with `--purge`, the volumes) it would remove.
* env values, install scripts and secrets are never printed - env by name, steps by name and flags.
* it queries the engine but creates nothing: no workspace volumes, networks or containers, no clone. A
  `--config git:...` source is fetched with a short-lived helper container, which is removed again.
* what it cannot know is shown as is: `skip_if` checks run in the container, so a step is listed with its
  flags and may still be skipped.

## Install steps

`install` runs shell steps in the work container when a workspace is created. A step is a string, or an
object:

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

* steps run in key order, as the **workspace user**. `root: true` runs a step as root (package managers,
  system files). A sidecar's steps run as its `uid` (root unless set).
* `skip_if` is a shell check that runs first, as the same user: exit 0 skips the step. Pair it with a
  [cache](#caching) so a tool downloaded once is not downloaded again. `run` and `skip_if` are templated.
* the check runs with the container's environment: a tool installed under a cache path must be on `PATH`
  (set it in `env`) or be checked by absolute path.
* layers (`bases`) add steps by key, replace a step as a whole, and remove one with `null`.
* `root: true` is refused in a configuration the operator did not author (an unlisted `--config git:...`
  source or `bases` layer); list the source in `ROOZ_TRUSTED_CONFIGS` or move the step into your own local
  config.

## Sidecars

A sidecar with `install` steps is built once: rooz runs the steps in a container and commits the
result as `localhost/rooz/<workspace>/<sidecar>:latest`, then starts the sidecar from that image.

The sidecar is created from the image **id** rooz got back from that commit, never from the name -
and the id is recorded on the sidecar container (`dev.rooz.runtime-image`), which is what a later
run checks before skipping the install stage. A name in the engine's image namespace is not evidence
of what carries it, and neither are the labels on it: anyone able to tag an image can write both. An
id is the image's content, so it cannot be pointed at somebody else's image. If the recorded id is
gone or no longer what rooz committed, the install stage runs again and the tag is rebuilt over.
Removing a workspace removes the images rooz built for it.

## Privileged containers

A privileged container has full access to the host running the container engine, so rooz does not
grant it on a config file's say-so. If `privileged: true` appears in a config file (a local or git
`--config` file, including its sidecars), workspace creation aborts and names the containers involved. To go
ahead, confirm explicitly with either:

* `ROOZ_ALLOW_PRIVILEGED=<names>` - a comma-separated list of the container names you are allowing,
  e.g. `ROOZ_ALLOW_PRIVILEGED=dkr`. Pure consent: the configuration is applied exactly as written,
  and anything *not* named is still refused.
* `ROOZ_ALLOW_PRIVILEGED=true` - allows whatever the configuration privileges. Convenient for CI,
  blunt everywhere else.
* `--privileged true` - consents *and* makes the work container privileged whether or not the
  configuration asked for it.

Name the containers rather than reaching for `true`. The common case is a container-engine sidecar
(docker-in-docker and friends): the daemon's container needs the privilege, the workspace talking to
it over `DOCKER_HOST` does not. Naming it keeps the setting safe to leave in your shell profile - a
standing `=true` would wave through a privileged container that a repository's own config file
slipped into the merged configuration, which is exactly what this check exists to stop.

⚠️ Only do this for configurations you trust. When opening repositories you do not control,
prefer rootless Podman, which confines the grant to your own user namespace.

## VPN sidecars

`vpn: true` on a sidecar gives it what WireGuard (`wg-quick`, kernel or `wireguard-go`), OpenVPN and
the like need to run inside it, and nothing more: the `NET_ADMIN` capability, the `/dev/net/tun`
device and `net.ipv4.conf.all.src_valid_mark=1`. All three are scoped to the sidecar's own network
namespace - unlike `privileged`, it does not reach the host's devices or network.

It still opens the kernel's netfilter and netlink interfaces to the sidecar, which on a rootful
engine is real root's reach into code with a history of container escapes. So, like `privileged`,
rooz does not grant it on a config file's say-so: name the sidecars with `ROOZ_ALLOW_VPN=<names>`
(or `ROOZ_ALLOW_VPN=true` for any). The consent is separate from `ROOZ_ALLOW_PRIVILEGED` - allowing
one does not allow the other.

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

* the sidecar must run as uid 0 (the default) - `NET_ADMIN` is not effective for a non-root user,
  so `vpn` with any other `uid` is refused
* kernel WireGuard needs the `wireguard` module loaded on the engine's host - a container cannot
  load it; without it `wg-quick` falls back to `wireguard-go` over `/dev/net/tun`
* full tunnels (`AllowedIPs = 0.0.0.0/0`) work: `wg-quick` finds `src_valid_mark` already set and
  the work container keeps reaching the sidecar over its pair network
* leave out `DNS =` - it calls `resolvconf`, which conflicts with the engine-managed `/etc/resolv.conf`
* the tunnel is the sidecar's: give the work container (or `peers:`) access through a proxy the
  sidecar runs, e.g. `HTTP_PROXY=http://vpn:3128`

## Files next to the config

`file:` takes a data value's content from a file instead of inlining it, the way a Kustomize config
map generator does. It works wherever `content:` does: `data`, inline `mounts` and sidecar `mounts`.

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

* the path is relative to the config that declares it (a base's files are relative to that base),
  below its directory: no absolute paths, no `..`
* a local config reads the file from disk; a config from git reads it from the same checkout,
  next to the declaring file
* symlinks are refused - the file, or any directory on the way to it
* at most 64 KiB, valid UTF-8 text. For anything bigger or binary use a volume or `generate:`
* the content is used as is - not templated. Anything that needs `{{ vars }}` belongs in `content:`
* the stored config keeps the reference, not the content

## Port mappings

Port mappings for the work container can be specified via a config file only:

```yaml
ports:
- "80:8080"
- "22:8022"
```

