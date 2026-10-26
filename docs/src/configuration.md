# Configuration

Settings come from env vars, flags and a YAML config file. The file covers most options; pass it with
`--config` on `rooz new` ([examples](https://github.com/queil/rooz/tree/main/examples)).

`--config` takes a local path or a file in a git repository, `<repo-url>//<path-in-repo>`, e.g.
`git@github.com:my/configs//path/in/repo/config.rooz.yaml`. Rooz clones the repository with its SSH key,
so only SSH URLs work: scp-like `git@host:repo` or `ssh://git@host[:port]/repo` (also `git+ssh://`,
`ssh+git://`). Other schemes are refused.

Rooz never reads config from a repository it clones. Pass it explicitly:

```sh
rooz new ws -g <url> --config <url>//.rooz.yaml
```

## Remote config

Config from a remote source (a `--config <url>//<path>`), and any config
that merges a `bases` layer not in `ROOZ_TRUSTED_CONFIGS`, may set the fields scoped to the workspace: `bases`, `git_ssh_url`, `image`, `shell`, `command`, `args`, `env`, `install`,
`ports`, `data`, `mounts`, `sidecars`, `vars`, `user`, `init`.

These are refused unless you allow them:

| Field | Why | To allow |
| --- | --- | --- |
| `secrets` | plaintext lands in a container the config controls | list the source in `ROOZ_TRUSTED_CONFIGS` ([Trusted config sources](secrets.md#trusted-config-sources)), or use a local `--config` file |
| `caches` | shared with your other workspaces | the same paths in `ROOZ_CACHES` (or `--caches`, which covers `rooz new` only, not `rooz update`), or the source in `ROOZ_TRUSTED_CONFIGS` |
| `extra_repos` | cloned with your ssh key into the workspace | the source in `ROOZ_TRUSTED_CONFIGS`, or a local `--config` file |
| `install` step with `root: true` | runs as root in the workspace | the source in `ROOZ_TRUSTED_CONFIGS`, or a local `--config` file ([Install steps](#install-steps)) |
| `privileged` | full host access | `ROOZ_ALLOW_PRIVILEGED=<name>@<fingerprint>`, required for *any* config file |
| sidecar `vpn` | kernel netfilter access as root on a rootful engine | `ROOZ_ALLOW_VPN=<name>@<fingerprint>`, required for *any* config file |

## Clone transports

Clone URLs (the root repo and `extra_repos`) accept `ssh://`, `git+ssh://`, `ssh+git://`, scp-like
`user@host:path`, `https://`, `http://`, `git://` and `file://`. Anything else is refused, including
`ftp://` and remote helpers (`name::address`, e.g. `ext::`). Remote `--config` sources take SSH URLs
only (see above).

`http://` and `git://` are unauthenticated, so a network attacker controls the clone. Rooz warns about
them; set `ROOZ_REQUIRE_SECURE_TRANSPORT=true` to refuse them.

## Image

Default: rooz's helper image, `ghcr.io/queil/joey:<version>-tools` pinned by digest (Alpine with
`sh`, `git` and `openssh-client`). Set it with `ROOZ_IMAGE`, `--image`, or in a config file:

```yaml
image: ghcr.io/queil/image:dotnet
shell: [bash]
```

## User

Rooz runs as uid `1000` (`--uid` on `rooz new` changes it). The image must have this user, named
`rooz_user` unless you set `ROOZ_USER` or `--user`. Names may contain letters, digits, `_`, `-` and `.`,
and must not start with `-` or `.`.

Home is `/home/<user>`: the SSH key volume mounts there, and `~/` in mount targets expands to it.

## Shell

Default: `sh` (`bash` for `rooz tmp`). Set it with:

* `ROOZ_SHELL`, read by `rooz new`, `rooz update` and `rooz tmp`
* `--shell` on `rooz enter` and `rooz tmp`
* `shell` in a config file, as a list, e.g. `[bash]`

## Caches

Shared caches, keyed by path:

```yaml
caches:
- ~/.nuget
```

Workspaces with the same cache path share one volume at that path. `ROOZ_CACHES` (comma-separated,
merged with the config) and `--caches` add more.

> [!WARNING]
> Cache contents (cargo sources, linker config, build scripts) execute in every workspace that mounts
> the path. So cache paths from remote config (a `--config <url>//<path>` source not in
> `ROOZ_TRUSTED_CONFIGS`) are refused unless you also list them in `--caches` (on `rooz new`) or
> `ROOZ_CACHES`. `rooz update` re-reads remote config on every run, so only `ROOZ_CACHES` covers it.
> Local `--config` files are not affected, unless they merge a `bases` layer not in
> `ROOZ_TRUSTED_CONFIGS`.

Cache volumes are scoped per operator, by default the uid running rooz, so operators sharing an engine
(a remote `DOCKER_HOST`, a multi-user socket) get separate volumes: toolchain caches hold executable
config such as `~/.cargo/config.toml` and `~/.m2/settings.xml`. If uids collide across machines on one
engine, give each operator a distinct `ROOZ_CACHE_SCOPE`. The scope is not access control: anyone who
can create workspaces on the engine can use any scope.

## Dry run

`--dry-run` on `rooz new` and `rooz update` prints the plan and changes nothing:

```
$ rooz new demo --config demo.rooz.yaml --dry-run
pull     image     docker.io/alpine:latest
create   volume    rooz_demo_work
reuse    cache     rooz_cache_uid-1000_---nuget  ~/.nuget
create   network   demo_egress
create   network   demo_net_db
create   sidecar   db  image=docker.io/postgres:17 install: 00-x[root]
create   work      demo  image=docker.io/alpine:latest user=rooz_user uid=1000 env=PGHOST
run      install   00-tools  root,skip_if

Dry run: nothing was changed.
```

* Config is resolved and checked as in a real run (bases, templating, secrets, consent, trust), with the
  same errors.
* For `update`, the plan also lists containers to remove, and volumes with `--purge`.
* Env values, install scripts and secrets are not printed; only env names and step names and flags.
* Rooz queries the engine but creates nothing: no volumes, networks, containers or clones. A
  `--config <url>//<path>` source is fetched in a temporary helper container.
* `skip_if` runs in the container, so a planned step may still be skipped.

## Install steps

`install` runs shell steps in the work container when the workspace is created. A step is a string or
an object:

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

* Steps run in key order as the **workspace user**, or as root with `root: true`. Sidecar steps run as
  the sidecar's `uid` (default root).
* `skip_if` runs first, as the same user; exit 0 skips the step. Pair it with a [cache](#caches) to
  avoid repeat downloads. `run` and `skip_if` are templated.
* `skip_if` sees the container's env: put cached tools on `PATH` (via `env`) or use absolute paths.
* Containers install side by side: sidecar images build while the work container's steps run. On a
  terminal each gets a pane with its current step and last output lines; a finished one collapses to a
  summary of its steps (`✓` ran, `○` skipped, `↺` a sidecar image reused), a failed one leaves its
  step's whole output. Elsewhere, or with `ROOZ_PROGRESS=plain`, every
  output line is printed tagged `[install:<container>:<step>]`.
* `bases` layers add steps by key, replace whole steps, or remove them with `null`.
* `root: true` is refused from untrusted config (a `--config <url>//<path>` source or `bases` layer not
  in `ROOZ_TRUSTED_CONFIGS`). Trust the source or use a local config.

## Sidecar images

A sidecar with `install` steps is built once: rooz runs the steps, commits
`localhost/rooz/<workspace>/<sidecar>:latest` and starts the sidecar from it.

* The sidecar runs the committed image by **id**, not by name, since tags and labels can be forged. The
  id is recorded on the sidecar container (`dev.rooz.runtime-image`).
* `rooz update` pulls the base image (unless `--no-pull`) and reuses the built image while the base
  image id, `uid`, `command`, `args` and install steps are unchanged, skipping the install.
  `rooz update --rebuild` builds it again anyway, e.g. when an install step that exits 0 left it
  broken; volumes are kept. A change to any of them, or `--purge`, builds it again from the
  base image.
* A rebuild starts from the base image, so `skip_if` sees nothing earlier builds installed. It only
  skips steps whose output is on a volume or cache. `rooz update --rebuild` ignores `skip_if` and
  runs every step, the work container's too, so keep steps safe to run again.
* Install steps run without the sidecar's `env`, as its `uid`. Use absolute paths there; `env` applies
  when the sidecar runs.
* Removing a workspace removes its built images.

## Privileged containers

`privileged: true` in any config file (local or git, sidecars included) stops workspace creation and
names the containers. To allow it:

* `ROOZ_ALLOW_PRIVILEGED=<name>@<fingerprint>` allows exactly that definition of the container
  (comma-separated for several), e.g. `ROOZ_ALLOW_PRIVILEGED=dkr@7f3fa966d667faa1`. The refusal prints
  the value to set. Others are still refused.
* The fingerprint covers the container's image, command, args, env, uid, install steps, mounts and the
  data behind them (secrets masked). Change any of them, or let another config declare its own `dkr`,
  and consent is needed again. A bare name (`dkr`) no longer consents.
* `ROOZ_ALLOW_PRIVILEGED=true` allows any. Meant for CI.
* `--privileged true` allows it *and* makes the work container privileged, whatever the config says.

Prefer pinned names over `true`: pin the docker-in-docker sidecar, not the workspace that uses it
through `DOCKER_HOST`. Pinned names are safe to keep in your shell profile, as they cover only the
definition you reviewed; a standing `=true` also lets remote config add privileged containers.

> [!WARNING]
> Only for configs you trust. For repositories you do not control, prefer rootless Podman, which
> confines the grant to your user namespace.

## VPN sidecars

`vpn: true` on a sidecar grants what WireGuard (`wg-quick`, kernel or `wireguard-go`), OpenVPN and the
like need: `NET_ADMIN`, `/dev/net/tun` and `net.ipv4.conf.all.src_valid_mark=1`. The grant is scoped to
the sidecar's network namespace; unlike `privileged`, it gives no host devices or host network.

It does expose kernel netfilter and netlink, an escape surface on rootful engines, so it requires
`ROOZ_ALLOW_VPN=<name>@<fingerprint>` (or `=true` for any), pinned like
[privileged consent](#privileged-containers). This is separate from `ROOZ_ALLOW_PRIVILEGED`.

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

* The sidecar must run as uid 0 (the default). `vpn` with another `uid` is refused: `NET_ADMIN` has no
  effect for non-root.
* Kernel WireGuard needs the `wireguard` module loaded on the engine host; containers cannot load it.
  Without it, `wg-quick` falls back to `wireguard-go`.
* Full tunnels (`AllowedIPs = 0.0.0.0/0`) work; the work container still reaches the sidecar over its
  pair network.
* Omit `DNS =`: `resolvconf` conflicts with the engine-managed `/etc/resolv.conf`.
* The tunnel belongs to the sidecar alone. Share it through a proxy in the sidecar, e.g.
  `HTTP_PROXY=http://vpn:3128` for the work container, or through `peers:`.

## Data and mounts

`data` declares volumes and files; `mounts` maps container paths to them:

```yaml
data:
  notes: {}                    # empty read-write volume
  gitconfig:
    content: |
      [url "git@github.com:"]
        insteadOf = gh:
  tls-key:
    generate: openssl genrsa 2048
    image: docker.io/alpine/openssl
    once: true
mounts:
  ~/notes: notes
  ~/.gitconfig: gitconfig
  ~/tls.key: tls-key
  ~/.bash_history:             # inline: no data key needed
    content: ""
    once: true
```

A `data` entry is one of:

* `{}`: an empty volume, `rooz_<workspace>_<key>`.
* `content:`: a file with this text.
* `file:`: a file loaded from next to the config. See [Files next to the config](#files-next-to-the-config).
* `generate:`: a file holding the stdout of a `sh` script, run in a one-shot container. `image:` picks
  the image (default: rooz's helper image, see [Image](#image)).

Files also take `executable: true` (mode `755`, else `644`) and `once: true`. They are owned by the user
of the container that mounts them, and writable.

A mount maps a path to a `data` key, or holds the same kinds inline, or a `dir:` of files (see
[Directories next to the config](#directories-next-to-the-config)). `~/` expands to the home directory
in the work container only. The `/work` volume is implicit. A missing directory above a mount in the work
container's home is created owned by the workspace user, so `~/.config/app.conf` leaves `~/.config`
writable.

A data key names its volume, `rooz_<workspace>_<key>`, with characters other than letters, digits and
`-` turned into `-`. Keys that end up alike (`a.b` and `a-b`), a key named `inline` and keys not
starting with a letter or digit are refused. Inline mounts are files in one shared volume, one per
container and path: the same path inline in the work container and a sidecar gives two files, and two
paths of one container that end up alike (`a.conf` and `a-conf`) are refused.

Sidecars use the same syntax. A key mounted in the work container and in a sidecar is one volume, so
they share it, e.g. a cluster's admin config written by a sidecar and read by the work container.

### Lifecycle

* Volumes (`{}`) keep their data across `rooz update`. `rooz rm` and `rooz update --purge` delete them.
* Files follow the config: `rooz update` writes every file again, overwriting changes made in the
  container, and runs `generate:` scripts again. Moving or renaming a workspace keeps them as they are.
* A `once` file is written only when nothing is at its path, then belongs to the containers: history,
  a generated key, a file a sidecar fills in. Its content is only the initial value; later config
  changes don't reach it.
* `content:` and `generate:` are [templated](templating.md); `file:` is not.

### Files next to the config

`file:` loads content from a file, like a Kustomize config map generator. It works wherever `content:`
does: `data`, inline `mounts` and sidecar `mounts`.

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

* The path is relative to the declaring config's directory (a base's own, for a base) and must stay
  below it: no absolute paths, no `..`.
* Git configs read files from the same checkout.
* Symlinks anywhere on the path are refused.
* Up to 64 KiB of UTF-8 text. For larger or binary data, use a volume or `generate:`.
* Not templated; use `content:` for `{{ vars }}`.
* The stored config keeps the reference, not the content.

### Directories next to the config

`dir:` mounts every file below a directory next to the config, each at the same relative path below
the mount's. The files go into the container one by one, so what is already at the mount path stays:

```yaml
mounts:
  ~/:
    dir: home                     # home/.bashrc -> ~/.bashrc, home/.config/nvim/init.lua -> ~/.config/nvim/init.lua
  ~/.config/nvim/init.lua:        # a path mounted on its own keeps that mount
    file: files/init-local.lua
```

* Inline `mounts` only, in the work container and sidecars; not in `data`.
* Every file is a `file:`, with the same rules: below the config's directory, no symlinks (anywhere in
  the directory either), UTF-8 text up to 64 KiB, names of letters, digits, `.`, `_` and `-`. Hidden
  files are included.
* At most 256 files and 1 MiB per directory.
* A file is executable if it is in the directory (git keeps the bit); `executable: true` or `false`
  sets it for all. `once: true` applies to each file.
* A path mounted on its own, in any layer, keeps that mount. Two `dir:` mounts providing the same path
  are refused.
* `rooz update` reads the directory again: new files are mounted, removed ones are gone.

## Ports

Work container ports, set in the config file only. The format is `[<host port>:]<container port>`, as
in Docker, bound to `127.0.0.1`:

```yaml
ports:
- "8080:80"
- "8022:22"
```
