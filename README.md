<p align="center">
    <img width="180" src="https://github.com/queil/rooz/blob/main/assets/rooz512.webp?raw=true" alt="Rooz">
</p>

<h1 align="center">rooz</h1>

Rooz is a CLI tool that enables you to work in containers. It is intended for developers and DevOps engineers. Because of that, it comes with a built-in support for git repositories, SSH keys generation, support for git-safe secrets, shared caches, sidecar containers, declarative configuration, and a robust CLI. Rooz is designed to share as little as possible with the host so it can also orchestrate your workspaces on remote Docker/Podman hosts.

## Requirements

Rooz requires one of the following container backends:
* Docker >= 29.5.0
* Podman >= 6
* Rancher Desktop - untested atm

The engine is assumed to be yours - a rootless daemon on your own machine, or a remote one only you
use. Rooz defends your host, your other workspaces and your secrets against the repositories you
open; it cannot defend a workspace against another tenant of the same engine. Anyone who can reach
the engine API can read every volume, run containers on your networks, and name images and volumes
whatever they like, so sharing one engine between people who do not trust each other is out of scope.

## Quick start

### Install

#### MacOs (Apple silicon)

```sh
curl -sSL https://github.com/queil/rooz/releases/latest/download/rooz-aarch64-apple-darwin -o ./rooz && chmod +x ./rooz && sudo mv ./rooz /usr/local/bin
```

#### MacOs (Intel)

```sh
curl -sSL https://github.com/queil/rooz/releases/latest/download/rooz-x86_64-apple-darwin -o ./rooz && chmod +x ./rooz && sudo mv ./rooz /usr/local/bin
```

#### Linux (x86_64)

```sh
curl -sSL https://github.com/queil/rooz/releases/latest/download/rooz-x86_64-unknown-linux-gnu -o ./rooz && chmod +x ./rooz && sudo mv ./rooz /usr/local/bin
```

#### Linux (arm64)

```sh
curl -sSL https://github.com/queil/rooz/releases/latest/download/rooz-aarch64-unknown-linux-gnu -o ./rooz && chmod +x ./rooz && sudo mv ./rooz /usr/local/bin
```

Every release also carries `SHA256SUMS`, and the binaries have signed build provenance
(`gh attestation verify ./rooz --repo queil/rooz`).

### Initialize

:warning: If you intend to use a remote Docker host with rooz please [configure it first](#connecting-to-a-remote-dockerpodman-host)

To initialize rooz run the following command:

```sh
rooz system init
```

The command creates:

* an SSH key pair (ed25519) intended to use for auth wherever SSH keys can be used (like github.com)
  
  The generated key gets stored in a volume and then mounted **read-only** under `~/.ssh` to all rooz
  workspace containers, so a workspace cannot overwrite or delete it. Note that workspace containers
  can still *read* the key - that is the point of mounting it - and the workspace image and user come
  from the repository's configuration. Do not open repositories you would not trust with that key.

* an age encryption key pair intended to use for encryption of sensitive config data (i.e. secrets)

  The identity is stored **on your machine**, at `~/.config/rooz/age.key` (`$XDG_CONFIG_HOME/rooz/age.key`
  if set, or wherever `ROOZ_AGE_KEY_FILE` points), mode `0600`. It is never written to the engine: a
  container engine hands the contents of every named volume to every one of its API users, and this key
  decrypts every secret you have ever encrypted with rooz - including ciphertext you commit to a
  repository. Only the CLI uses it; nothing inside a container ever needs it.

  :information_source: It's important to back up the generated age identity - copy that file somewhere safe.
  If the key is lost all the existing config files with encrypted vars won't decrypt and re-encrypting will be required.
  To init rooz with an existing age identity use the `--age-identity` switch. Working from a second machine
  against the same engine means copying the identity file there.

  :information_source: Upgrading: earlier versions kept the identity in the `rooz_sys-config` volume. The first
  rooz command after upgrading takes it out of the volume and keeps it aside at `age.engine.bak`, printing
  where it went - it is **not** adopted as this machine's identity, because rooz cannot tell your own
  upgraded key from one somebody else with engine access wrote there, and using theirs would encrypt
  every secret you save afterwards to their recipient. Check the file, then install it yourself:
  `rooz system init --force --age-identity "$(cat ~/.config/rooz/age.engine.bak)"`. If that engine is
  shared with anyone, treat the identity as compromised instead: re-key with `rooz system init --force`
  and re-encrypt your secrets.

You can regenerate the age identity by specifying the `--force` parameter. Please note that the existing
age key will be wiped out.

The SSH key pair is deliberately kept across re-inits - every existing workspace mounts it - so `--force`
does not touch it. To replace it (the remediation if you believe it leaked) run:

```sh
rooz system init --rotate-ssh-key
```

That generates a new pair in place and prints the new public key. Register it wherever you used the old
one and de-register the old one; running workspaces pick the new key up on their next start.

:warning: The ssh key is the engine's single git identity and every workspace can read it (see above), so
a repository you open can take it. Rotation limits the damage after the fact; it does not prevent the
theft. Treat opening an untrusted repository as handing over that key.

### Configure

:information_source: Read more in the [Configuration](#configuration) section

Rooz works best with user's provided custom image(s).
You can pass image, shell, and user via cli parameters but it's much more convenient to set it up 
in your host's `~/.bashrc` (or a non-bash equivalent), and only override it via cli when required.

#### Example configuration:

```sh
export ROOZ_USER=queil
export ROOZ_IMAGE=ghcr.io/queil/image:latest
export ROOZ_SHELL=bash
export ROOZ_CACHES='~/.local/share/containers/storage/'
```

### System config

Rooz exposes some system config via `rooz system configure`. At present it only allows to specify `.gitconfig` used by rooz for cloning. It can be used e.g. to specify aliases. This feature is new and may be extended in the future. The age identity is *not* part of it - that lives on your machine (see [Init](#init)).

## Usage examples

### Create an empty workspace

```sh
rooz new myworkspace
```

### Create a workspace from a git repo

```sh
rooz new -g git@github.com:your/repo myworkspace2
```
### 

### Enter a previously created workspace

```sh
rooz enter myworkspace2
```

### Run one command in a workspace

```sh
rooz exec myworkspace2 -- cargo test
```

Output is passed through (stdout and stderr stay separate), there is no tty or stdin, and `rooz`
exits with the command's exit code. `--root`, `--work-dir` and `--container` (a sidecar) work as in `enter`.

### Inspect workspaces

```sh
rooz list [--json]                       # status, image, age, config origin
rooz status myworkspace2 [--json]        # containers, volumes, config origin
rooz logs myworkspace2 [-c sidecar] [-f] [--tail 100]
```

### Editor support and inspecting merged config

```sh
rooz config schema > rooz.schema.json              # JSON Schema of the config file
rooz config show myworkspace --resolved            # merged settings, one row each, with the layer it came from
```

Point a YAML language server at the schema for completion and typo detection, either per file with
a `# yaml-language-server: $schema=./rooz.schema.json` first line or through your editor's
`yaml.schemas` setting. `--resolved` merges the stored defaults, `bases` and config the way a run
does; templates stay unexpanded and secret values are shown as `***`. Settings given as flags to
`rooz new` are not recorded and so not shown.

### Check the setup

```sh
rooz doctor
```

Checks that the engine is reachable and recent enough, that `rooz system init` was done (system
config and ssh key volumes, age identity on this machine) and, on docker, how much of the default
network address pool is used. Exits non-zero when something is missing.

### Manage caches

```sh
rooz cache ls [--all-scopes] [--json]   # size, age, mounting workspaces
rooz cache rm '~/.cargo'                # by path, or by name as shown in 'ls'
rooz cache prune [--dry-run]            # removes every cache no workspace mounts
```

`rm` and `prune` only touch your own scope (see `ROOZ_CACHE_SCOPE`) and refuse caches that a
container still mounts. Size shows `-` on engines without a disk usage report.

### Interactive shell in an anonymous ephemeral workspace

```sh
rooz tmp --image alpine --shell sh
```

## Configuration

:information_source: Rooz supports `yaml` as configuration formats.

Most of the settings can be configured via:

* environment variables
* a config file specified via `--config` (on `rooz new`)
  :information_source: it can be a local file path or a remote git file like: `git@github.com/my/configs//path/in/repo/config.rooz.yaml`

  :information_source: rooz does not look for configuration in the repositories it clones. To use a
  repository's own config file, pass it explicitly: `rooz new ws -g <url> --config <url>//.rooz.yaml`
* cmd-line parameters

The configuration file provides the most options: [example](examples/dotnet-nats.rooz.yaml)

### What a repository's configuration may set

Configuration authored by the repository being opened (a `--config git:...` source, such as its
`.rooz.yaml`) may set anything that only shapes the workspace container - `image`, `shell`, `command`,
`args`, `env`, `install`, `ports`, `data`, `mounts`, `sidecars`, `extra_repos`, `vars`, `user`, `init`. That
grants the repository nothing it does not already have: the workspace runs its code either way.

Fields that reach past the workspace - your host, your other workspaces, your secrets - are yours to set:

| field | why | how to allow it anyway |
| --- | --- | --- |
| `secrets` | plaintext would land in a container the repository controls | list the source in `ROOZ_TRUSTED_CONFIGS` if it is yours (see [Trusted config sources](#trusted-config-sources)), or move the settings into your own local `--config` file |
| `caches` | a cache volume is shared with your other workspaces | list the same paths in `--caches` / `ROOZ_CACHES`, or list the source in `ROOZ_TRUSTED_CONFIGS` |
| `privileged` | full access to the host running the engine | `ROOZ_ALLOW_PRIVILEGED=<names>` (refused on *any* config file's say-so, including your own) |
| sidecar `vpn` | `NET_ADMIN` and `/dev/net/tun` - real root's reach into the kernel's netfilter code on a rootful engine | `ROOZ_ALLOW_VPN=<names>` (refused on *any* config file's say-so, including your own) |

### Clone transports

Clone URLs (the root repo, `extra_repos`, `--config git:...`) may use `ssh://`, `git+ssh://`,
the scp-like `user@host:path`, `https://`, `http://`, `git://` or `file://`. Anything else -
`ftp://`, invented schemes, and `name::address` remote helpers such as `ext::` (which hands git
a command to run) - is refused.

`http://` and `git://` authenticate nothing: whoever sits on the network path decides what the
clone contains, including any `--config` file rooz reads from it. Those clones
print a warning. Set `ROOZ_REQUIRE_SECURE_TRANSPORT=true` to refuse them outright.

### Images

:information_source: the default image is `docker.io/chainguard/git:latest-dev`

There are a few ways of specifying images:
* via the `ROOZ_IMAGE` env variable
* via  the `--image` cmd-line parameter
* via `image` in a config file (`--config`):

  ```yaml
  image: ghcr.io/queil/image:dotnet
  shell: bash
  ```

### User

`rooz` runs as uid `1000` (always - it's hard-coded) so make sure it exists in your image
(with `rooz_user` as the name - it can be overridden via `ROOZ_USER` or `--user`)

The name must be a plain account name (letters, digits, `_`, `-`, `.`, not starting with `-` or
`.`): it becomes the workspace home `/home/<user>`, which is where the ssh key volume mounts and
what `~/` mount targets expand to.

### Shell

The default shell is `bash` but you can override it via:

* `ROOZ_SHELL` env var
* `--shell` cmd-line parameter (on `rooz enter`)
* in a config file via `shell`

### Caching

`rooz` supports basic path-keyed shared caches. It can be set in a config file like:

```yaml
caches: 
- ~/.nuget
```

All the workspaces specifying a cache path will share a container volume mounted at that path enabling cache reuse.
It also can be set globally via `ROOZ_CACHES` (comma-separated paths), or per workspace with `--caches`.
The global paths get combined with the config file's paths.

:warning: A cache is shared with *every* workspace of yours mounting the same path, so whatever writes
into it (a cargo registry source, a linker config, a crate with a build script) later executes in those
workspaces. That makes a cache path an operator decision: if the configuration comes from the repository
being opened (a `--config git:...` source not listed in `ROOZ_TRUSTED_CONFIGS`), rooz refuses its cache paths
unless you list the same paths yourself with `--caches` (on `rooz new`) or `ROOZ_CACHES` - the latter also
covers `rooz update`, which re-reads a repository-authored config on every run. Config files you wrote
locally and pass with `--config` are unaffected.

:information_source: Cache volumes are scoped to the operator, not just the path. The scope defaults to the
uid of the rooz process, so one person's workspaces share their caches while unrelated operators on a
*shared* engine (a remote `DOCKER_HOST`, a multi-user socket) get separate volumes. Toolchain caches hold
configuration that the toolchain executes (`~/.cargo/config.toml`, `~/.m2/settings.xml`, ...), so sharing
them across operators means sharing code execution.

If several machines share one remote engine and their local uids collide (both `1000`, say), set a
distinct `ROOZ_CACHE_SCOPE` per operator. Note that the scope is a collision boundary, not an
authorization boundary: anyone able to create workspaces on an engine can name any scope.

### Dry run

`rooz new <name> --dry-run` and `rooz update <name> --dry-run` report what the command would do and change
nothing:

```
$ rooz new demo --config demo.rooz.yaml --dry-run
pull     image     docker.io/alpine:latest
create   volume    rooz-demo-work
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

### Install steps

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

### Sidecars

A sidecar with `install` steps is built once: rooz runs the steps in a container and commits the
result as `localhost/rooz/<workspace>/<sidecar>:latest`, then starts the sidecar from that image.

The sidecar is created from the image **id** rooz got back from that commit, never from the name -
and the id is recorded on the sidecar container (`dev.rooz.runtime-image`), which is what a later
run checks before skipping the install stage. A name in the engine's image namespace is not evidence
of what carries it, and neither are the labels on it: anyone able to tag an image can write both. An
id is the image's content, so it cannot be pointed at somebody else's image. If the recorded id is
gone or no longer what rooz committed, the install stage runs again and the tag is rebuilt over.
Removing a workspace removes the images rooz built for it.

### Privileged containers

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

:warning: Only do this for configurations you trust. When opening repositories you do not control,
prefer rootless Podman, which confines the grant to your own user namespace.

### VPN sidecars

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

### Files next to the config

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

### Port mappings

Port mappings for the work container can be specified via a config file only:

```yaml
ports:
- "80:8080"
- "22:8022"
```

## Variables/templating

Rooz supports basic variable replacement/templating:
* [handlebars](https://handlebarsjs.com/guide/) syntax is used
* variables are declared via `vars`
* secrets are declared via `secrets`
* secrets are decrypted before expanding
* handlebars can be used in the most of the config values but not in YAML keys
* vars/secrets replacement works within `vars` themselves too. However, only if the var usage is 
below the var definition (in the document order).
* the secret section does not support var/secrets replacement

```yaml
secrets:
  sqlPassword: '----BEGIN AGE ENCRYPTED FILE-----|YWdlLWVuY3J ... truncated'

vars:
  sqlUser: admin
  sqlConnectionString: "uid={{ sqlUser }};pwd={{ sqlPassword }}"

env:
  SQL_CONNECTION_A: "database=A;{{ sqlConnectionString }}"
  SQL_CONNECTION_B: "database=B;{{ sqlConnectionString }}"

sidecars:
  tool:
    env:
      SQL_USER: "{{ sqlUser }}"
      SQL_PASSWORD: "{{ sqlPassword }}"

```

## Secrets

Rooz uses [age](https://github.com/C2SP/C2SP/blob/main/age.md) encryption to safely store
sensitive data (like API keys) in config files.

### Example

Make sure your rooz has been initiated with `rooz system init`

Run `touch example.yaml` and `rooz config edit example.yaml`. Your default editor should open.

Paste the following config, save & quit.

```yaml
secrets:
  apiKey: 1744420283158995

env:
  API_URL: https://api.rooz.dev/v1
  API_KEY: "{{ apiKey }}"

```

Now `cat example.yaml` and you should see something like the below:

```yaml
secrets:
  apiKey: '----BEGIN AGE ENCRYPTED FILE-----|YWdlLWVuY3J ... truncated'
env:
  API_URL: https://api.rooz.dev/v1
  API_KEY: "{{ apiKey }}"

```

Now, we can create a new workspace like:

```sh
rooz new secrets-test --config ./example.yaml
```

And enter the container and the `API_KEY` var is value gets decrypted and injected:

```sh
rooz enter secrets-test

> echo $API_KEY
1744420283158995
```

### Where secrets may be used

Expanding a secret needs two things: a configuration that is yours, and your say-so.

**Your say-so.** A secret is expanded only into a configuration you have released it to. Without
that, `rooz new` aborts and names the secrets involved. Consent with either:

* `ROOZ_ALLOW_SECRETS=<names>` - a comma-separated list of the secrets you are releasing, e.g.
  `ROOZ_ALLOW_SECRETS=dbPassword`. Anything *not* named is still refused, which makes this safe to
  leave in your shell profile, and it also covers `rooz update` (which re-renders the configuration
  on every run).
* `ROOZ_ALLOW_SECRETS=true` - releases whatever the configuration declares. Convenient for CI.
* `--allow-secrets true` - the same, for a single `rooz new` or `rooz update`.

The reason is that a local `--config` path is not proof of authorship: a repository can ship a config
file and its README can tell you to pass it (`rooz new ws --config ./setup.yaml`), and that file then
picks the image, entrypoint, install steps and environment your plaintext lands in. Naming the
secrets keeps a standing consent from covering a file you did not write.

**A configuration that is yours.** Secrets are expanded only when the **whole** configuration is one
you wrote - a local file passed with `--config`, with nothing else merged into it, or a source you
listed as yours in `ROOZ_TRUSTED_CONFIGS`. Rooz refuses to expand secrets and fails with an explanation
if the workspace also merges:

* a `--config git:...` source you have not listed - it may be the repository's own configuration;
* a `bases` layer outside a trusted directory. A base layer is a separate file your config points at;
  it may be a team overlay or something copied out of a repository, and after the merge nothing can
  tell its fields apart from yours.

The reason: every templated field - `env`, `install`, `command`, `shell`, `data` content, sidecar
`env` - can be written by whoever authored that content. A layer shipping `env: {LEAK: "{{ MY_SECRET }}"}`
would otherwise receive your decrypted secret inside a container whose image and entrypoint it also
controls. `vars` keep working everywhere, since they are not sensitive.

:warning: Consent is a gate, not a review. Rooz cannot tell your own `--config` file from one you
copied out of a repository you are opening, so releasing a secret to a configuration means you have
read it. Never point `--config` at a file that came from the repository, and read a base layer before
you include it.

#### Trusted config sources

Encrypting a secret to your age identity does not tie it to your configuration: anyone who has seen the
ciphertext can paste it into a config of their own, and rooz would decrypt it for them. So rooz decides
what is yours by where the configuration comes from, and you tell it:

```sh
export ROOZ_TRUSTED_CONFIGS='git@github.com:me/rooz-configs,~/rooz-configs'
```

* a repository URL - a `--config <url>//<path>` from it counts as yours, including its `bases` (they are
  read from the same clone). Matched exactly as you pass it to `--config`: `.../configs` and
  `.../configs.git` are different entries. Never trusted over `http://` or `git://`, where anyone on the
  network path can substitute the content.
* a local directory - a `--config` file counts as yours, `bases` included, when the file and every base it
  pulls in resolve (symlinks followed) inside it.

A trusted source gets what your own local file gets: secrets (still subject to `ROOZ_ALLOW_SECRETS`)
and shared caches. Trusting a repository means trusting everyone who can push to it - list only
repositories that are yours alone.

### Secrets at rest

Decrypted secret values are never written to the workspace-config volume. The container's environment
and generated files are populated from the real values at creation time, but the persisted
`runtime.config` - the file `rooz config show <ws> --part runtime` prints - has every occurrence of a
secret replaced with `***`. Config files in that volume are written owner-only.

:information_source: `shell` is the one field left unmasked, because `rooz enter` executes it. Do not
reference secrets from `shell`.

## Sidecars

*It's similar to docker-compose but super simple and limited to bare minimum.*

* `rooz` has a limited support for sidecars (containers running along). It is only available via the config file

  ```yaml
  sidecars:
    sql:
      image: my:sql
      command:
      - --some
      env:
        TEST: "true"
  
    tools:
      image: my:tools
  
  ```
  `image` may be omitted in individual config layers (e.g. an overlay extending a sidecar defined in a base via `bases:`) but must be set for each sidecar once all layers are merged, otherwise creating the workspace fails.

### Networking

Workspace networking follows a hub-and-spoke model:

* the work container can reach every sidecar — it shares a dedicated pair network with each one
* a sidecar can always reach the work container (pair-network reachability is symmetric; keep listeners in the work container minimal)
* sidecars cannot reach each other unless explicitly connected via `peers:`
* workspaces are isolated from each other

`peers:` lists other sidecar names a sidecar may talk to. Each declared relation gets its own network, `a: peers [b]` and `b: peers [a]` are equivalent, and peer names must refer to sidecars defined in the same workspace. Sidecars resolve each other by name over their peer network:

  ```yaml
  sidecars:
    claude:
      peers: [proxy]   # claude's only egress path is the proxy
    proxy:
      egress: true     # reachable only by the work container and claude
    dkr:
      peers: [images]  # docker-in-docker pulling via the mirror
    images:
      egress: true     # registry pull-through cache
  ```

  Unlike other list fields (replaced by higher config layers), `peers` merge as a deduplicated union across layers: an overlay can add reachability edges but cannot remove inherited ones.

  :warning: rooz creates one network per sidecar plus one per peer relation. Docker's default address pools allow only ~31 networks host-wide; if network creation fails with an address-pool error, configure `default-address-pools` with a smaller subnet size (e.g. `"size": 24`) in `daemon.json`. Podman (netavark) is not affected.

  The work container addresses sidecars by their names (`sql` and `tools` in the first example above). The usual container ID and IP work too, but are not as convenient.

* the `enter` command lets you specify `--container` to enter (otherwise it enters the work container).

Supported keywords:
* `image` - set containers image
* `env` - set environment variables
* `command` - override container entrypoint (`ENTRYPOINT` in Dockerfile)
* `args` - override container entrypoint arguments (`CMD` in Dockerfile)
* `mounts` - mount automatically-named rw volumes at the specified paths (so they can survive container restarts/deletes). It supports:

```yaml
  mounts:
  # simple empty rw volumes
  - /my_test/data/
  # config files with a specified content
  - mount: /etc/tool/
    files:
      config.json: |-
        {"some": "config"}
      other.yaml: |-
        some: config
```

* `ports` - port bindings in the `"8080:8080"` format
* `vpn` (`bool`) - grants `NET_ADMIN`, `/dev/net/tun` and `src_valid_mark` for in-sidecar VPNs, with operator consent (see [VPN sidecars](#vpn-sidecars))
* `peers` - names of other sidecars this sidecar may reach (see [Networking](#networking); merged as a union across config layers)
* `work_dir` - set working directory
* `mount_work` (`bool`) - if true then the work volume is mounted at `/work`

## Other facts

* cloned git repos are under `/work/{repo_name}` where `repo_name` is the default one generated by `git` during cloning.
* `rooz new`/`update` print one `[phase]` line per step (image, volumes, clone, container, install) on stderr. `-v`/`--verbose` adds debug logging; `RUST_LOG=rooz` still works

* if `rooz` misbehaves you can go nuclear and run `rooz system prune` to remove ALL the rooz containers and volumes. You can also remove just the workspaces, (leaving shared caches volumes, and the ssh volume untouched), by: `rooz rm --all --force`

  :warning: `rooz system prune` deletes all your state held with `rooz` so make sure anything important is stored before.

## Upgrading

* cache volumes gained an operator scope in their name (`rooz_cache_<scope>_<path>`), so caches created
  by earlier versions are orphaned on first use of the new version. Caches are disposable - they get
  repopulated, and `rooz system prune` removes the leftovers.

## Known issues

* auto-resizing rooz session to fit the terminal window (if resized) is not implemented. Workaround: exit the session, resize the window to your liking, enter the container.

## Connecting to a remote Docker/Podman host

Rooz connects to a local Docker daemon by default. It can also work with any number of remote
hosts at the same time: `rooz remote` forwards each host's Docker/Podman socket over SSH to a
local unix socket, and `-H <host>` points a rooz command at one of them.

### Preparation

#### The remote host

Rooz needs the engine socket path. Either give it to rooz when adding the host (`--socket`, below),
or export it at the top of `~/.bashrc` on the host (it is read by a non-interactive `ssh` command):

  * For Docker as root: `export DOCKER_HOST=/var/run/docker.sock`
  * For rootless Docker: `export DOCKER_HOST=/run/user/1000/docker.sock` (assuming your-user has uid=1000)
  * For Podman: `export DOCKER_HOST=/run/user/1000/podman/podman.sock` (assuming your-user has uid=1000)

#### The local host

Define your hosts (stored in `~/.config/rooz/remotes.yaml`, or `$XDG_CONFIG_HOME/rooz/remotes.yaml`):

```sh
rooz remote add box1 ssh://your-user@box1
rooz remote add box2 ssh://your-user@box2 --socket /run/user/1000/podman/podman.sock
rooz remote ls
```

Then keep this running (e.g. in a spare terminal or tmux):

```sh
rooz remote          # connects to all configured hosts
rooz remote box1     # or just some of them
```

Each host's engine socket is forwarded to `$XDG_RUNTIME_DIR/rooz/<host>.sock` (`~/.rooz/<host>.sock`
if `XDG_RUNTIME_DIR` is not set). Hosts are independent: one being down or reconnecting does not
affect the others. Only one `rooz remote` can serve a given host at a time.

### Working with a host

```sh
rooz -H box1 new myws        # one-off
export ROOZ_HOST=box2        # per shell: every rooz command in it targets box2
rooz list                    # lists box2's workspaces
rooz list --all-hosts        # workspaces of every connected host
rooz -H box1 code myws       # attaches VsCode to the workspace on box1
```

Precedence: `-H` / `ROOZ_HOST`, then `DOCKER_HOST`, then the local engine. There is deliberately no
global "current host": use one terminal (or `ROOZ_HOST`) per host. `rooz system prune` and
`rooz remove --all` print the target they are about to hit.

Workspaces, volumes, SSH keys and caches live on each host's engine, so the same workspace name on
two hosts is two unrelated workspaces. The age identity and `ROOZ_TRUSTED_CONFIGS` are local and
apply to every host.

### Port forwarding

If any remote containers publish ports listed in their `forward-ports` label, `rooz remote` forwards
them to the same port on `127.0.0.1`. Local ports are shared by all hosts: if a port is already bound
by another process or forwarded for another host, it is skipped with a warning.

### VsCode

`rooz code` starts `code` with `DOCKER_HOST` set to the selected host's socket. If VsCode is already
running it may ignore that and use its own `docker.host` setting instead:

```json
   "docker.host": "unix:///run/user/1000/rooz/box1.sock"
```

## Running with Podman

1. Make sure podman remote socket is enabled:

`podman info` should contain the following YAML:

```yaml
remoteSocket:
    exists: true
    path: /run/user/1000/podman/podman.sock
```

If `exists: true` is missing, try this command: `systemctl --user enable --now podman.socket`

2. Make sure you have the podman socket exposed as the `DOCKER_HOST` env var like:

```
export DOCKER_HOST=unix:///run/user/1000/podman/podman.sock
```

3. Use fully-qualified image names or define unqualified-search registries `/etc/containers/registries.conf`

4. When running more complex podman in podman scenarios (like networking) you may need to run rooz with `--privileged` switch
   [more info](https://www.redhat.com/sysadmin/privileged-flag-container-engines).

## Resources

* [my image I use with rooz](https://github.com/queil/image/blob/main/src/Containerfile)
