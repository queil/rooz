# Usage

## Create a workspace

```sh
rooz new myworkspace                                # empty
rooz new -g git@github.com:your/repo myworkspace2   # from a git repo
```

## Enter a workspace

```sh
rooz enter myworkspace2
```

## Run one command in a workspace

```sh
rooz exec myworkspace2 -- cargo test
```

No tty or stdin. stdout and stderr pass through separately, and rooz exits with the command's exit code.
`--root`, `--work-dir` and `--container` (a sidecar) work as in `enter`.

## Inspect workspaces

```sh
rooz list [--json]                       # status, image, age, config origin
rooz status myworkspace2 [--json]        # containers, volumes, config origin
rooz logs myworkspace2 [-c sidecar] [-f] [--tail 100]
rooz egress log myworkspace2 [-c sidecar] [--denied] [--suggest] [-f]   # see Sidecars: Egress log
```

## Schema and merged config

```sh
rooz config schema > rooz.schema.json              # JSON Schema of the config file
rooz config show myworkspace --resolved            # merged settings, each with the layer it came from
```

* For editor completion and validation, start a config file with
  `# yaml-language-server: $schema=./rooz.schema.json`, or set your editor's `yaml.schemas`.
* `--resolved` merges stored defaults, `bases` and config the way a run does. Templates stay unexpanded
  and secrets show as `***`. Flags passed to `rooz new` are not recorded, so they do not appear.

## Start a config from your project

```sh
rooz config template --detect > ws.rooz.yaml   # or --detect path/to/project
```

Prints a commented starter config. `--detect` picks the image and the toolchain's cache paths from:

| File                                    | Image                                       |
|-----------------------------------------|---------------------------------------------|
| `Cargo.toml`                            | `rust:1`                                    |
| `package.json`                          | `node:lts`                                  |
| `go.mod`                                | `golang:1`                                  |
| `pyproject.toml` / `requirements.txt`   | `python:3`                                  |
| `*.sln` / `*.csproj`                    | `mcr.microsoft.com/dotnet/sdk:<highest TargetFramework>`, default `10.0` |

Floating tags get updates through `rooz update`. .NET is pinned because an SDK image carries only its
own runtime. For a project with several stacks, the first one found sets the image; caches are listed
for all of them.

## Rename a workspace

```sh
rooz move myworkspace newname [--dry-run]
```

Volumes cannot be renamed, so rooz stops the workspace, copies its work and data volumes to the new
name, rebuilds it from its stored config and removes the old one.

* The new name must be free; nothing is overwritten.
* Secrets need the same consent as for `rooz update`.
* Needs about twice the workspace's disk space. Install steps run again; processes in the workspace end.
* On failure, the partial new workspace is removed and the old one kept.
* `rooz enter` the new name afterwards, as after `rooz new`.

## Move a workspace to another host

```sh
rooz move myworkspace --to buildbox [newname] [--remove-source] [--dry-run]
```

Moves from the current engine (`-H`/`DOCKER_HOST`) to a host from `rooz remote`. Both must be
connected.

* Volumes stream through this machine as tar, preserving owners, modes and symlinks, without buffering
  in memory.
* The target is rebuilt with `rooz new` from the workspace's config file, which must still be on this
  machine.
* The source is left stopped. Remove it with `--remove-source`, or later with `rooz rm`.
* On failure, the partial target is removed and the source restarted.
* Caches, images and the SSH key are not moved. Run `rooz system init` on the target and register its
  public key.
* Rooz warns if the target CPU architecture differs: compiled artifacts will not run there.

## Check the setup

```sh
rooz doctor
```

Checks that the engine is reachable and recent enough, and that `rooz system init` has run (system
config and SSH key volumes, age identity on this machine). On Docker, it also reports usage of the
default network address pool. Exits non-zero if anything is missing.

## Manage caches

```sh
rooz cache ls [--all-scopes] [--json]   # size, age, mounting workspaces
rooz cache rm '~/.cargo'                # by path, or by name as shown in 'ls'
rooz cache prune [--dry-run]            # removes every cache no workspace mounts
```

`rm` and `prune` act on your own scope only (see `ROOZ_CACHE_SCOPE`). `rm` refuses a cache a container
mounts; `prune` leaves it. Size shows `-` on engines that do not report disk usage.

## Throwaway shell

An anonymous workspace, removed when you exit:

```sh
rooz tmp --image alpine --shell sh
```
