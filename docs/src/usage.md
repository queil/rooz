# Usage examples

## Create an empty workspace

```sh
rooz new myworkspace
```

## Create a workspace from a git repo

```sh
rooz new -g git@github.com:your/repo myworkspace2
```

## Enter a previously created workspace

```sh
rooz enter myworkspace2
```

## Run one command in a workspace

```sh
rooz exec myworkspace2 -- cargo test
```

Passes stdout and stderr through separately, with no tty or stdin, and exits with the command's exit code.
* `--root`, `--work-dir` and `--container` (a sidecar) work as in `enter`.

## Inspect workspaces

```sh
rooz list [--json]                       # status, image, age, config origin
rooz status myworkspace2 [--json]        # containers, volumes, config origin
rooz logs myworkspace2 [-c sidecar] [-f] [--tail 100]
```

## Editor support and inspecting merged config

```sh
rooz config schema > rooz.schema.json              # JSON Schema of the config file
rooz config show myworkspace --resolved            # merged settings, one row each, with the layer it came from
```

* Use the schema with a YAML language server: add a `# yaml-language-server: $schema=./rooz.schema.json`
  first line, or set your editor's `yaml.schemas`.
* `--resolved` merges stored defaults, `bases` and config as a run does. Templates stay unexpanded;
  secret values show as `***`. Flags given to `rooz new` are not recorded, so not shown.

## Start a config from your project

```sh
rooz config template --detect > ws.rooz.yaml   # or --detect path/to/project
```

Prints a commented starter config. `--detect` sets the image and the toolchain's cache paths from:

| File                                    | Image                                       |
|-----------------------------------------|---------------------------------------------|
| `Cargo.toml`                            | `rust:1`                                    |
| `package.json`                          | `node:lts`                                  |
| `go.mod`                                | `golang:1`                                  |
| `pyproject.toml` / `requirements.txt`   | `python:3`                                  |
| `*.sln` / `*.csproj`                    | `mcr.microsoft.com/dotnet/sdk:<highest TargetFramework>`, default `10.0` |

`rooz update` re-pulls floating tags. .NET is pinned because the SDK image only has its own runtime.
With several stacks, the first picks the image; caches of all are listed.

## Rename a workspace

```sh
rooz move myworkspace newname [--dry-run]
```

* Stops the old workspace, copies its work and data volumes to the new name (volumes cannot be
  renamed), rebuilds it from its stored config and removes the old one.
* Secrets need the same consent as `rooz update`.
* The new name must be free; it never replaces.
* On failure, the partial new workspace is removed and the old one is kept.
* Needs about twice the workspace's disk space. Install steps rerun; workspace processes end.
* Afterwards, `rooz enter` it (as after `rooz new`).

## Move a workspace to another host

```sh
rooz move myworkspace --to buildbox [newname] [--remove-source] [--dry-run]
```

* Source: the current engine (`-H`/`DOCKER_HOST`). Target: a host from `rooz remote`. Both must be
  connected.
* Volumes stream through this machine as tar (owners, modes, symlinks preserved; not buffered in memory).
* Rebuilt on the target with `rooz new` from its config file, which must still be on this machine.
* The source is left stopped. `--remove-source` removes it; otherwise `rooz rm` it yourself.
* On failure, the partial target workspace is removed and the source is restarted.
* Not moved: caches, images, the SSH key (run `rooz system init` on the target and register its
  public key).
* Warns if the target CPU architecture differs: compiled artifacts will not run.

## Check the setup

```sh
rooz doctor
```

Checks that:

* the engine is reachable and recent enough
* `rooz system init` was run (system config and SSH key volumes, age identity on this machine)
* on Docker: usage of the default network address pool

Exits non-zero if something is missing.

## Manage caches

```sh
rooz cache ls [--all-scopes] [--json]   # size, age, mounting workspaces
rooz cache rm '~/.cargo'                # by path, or by name as shown in 'ls'
rooz cache prune [--dry-run]            # removes every cache no workspace mounts
```

`rm` and `prune` only affect your own scope (see `ROOZ_CACHE_SCOPE`) and refuse caches mounted by a
container. Size shows `-` on engines without disk usage reporting.

## Interactive shell in an anonymous ephemeral workspace

```sh
rooz tmp --image alpine --shell sh
```
