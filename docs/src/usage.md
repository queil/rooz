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

Output is passed through (stdout and stderr stay separate), there is no tty or stdin, and `rooz`
exits with the command's exit code. `--root`, `--work-dir` and `--container` (a sidecar) work as in `enter`.

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

Point a YAML language server at the schema for completion and typo detection, either per file with
a `# yaml-language-server: $schema=./rooz.schema.json` first line or through your editor's
`yaml.schemas` setting. `--resolved` merges the stored defaults, `bases` and config the way a run
does; templates stay unexpanded and secret values are shown as `***`. Settings given as flags to
`rooz new` are not recorded and so not shown.

## Start a config from your project

```sh
rooz config template --detect > ws.rooz.yaml   # or --detect path/to/project
```

Prints a commented starter config. With `--detect` it looks for `Cargo.toml`, `*.sln`/`*.csproj`,
`package.json`, `go.mod` and `pyproject.toml`/`requirements.txt` in that directory and fills in an
image and the cache paths of the toolchain. Images are the official ones on floating tags
(`rust:1`, `node:lts`, `golang:1`, `python:3`), which `rooz update` re-pulls, so they stay current
without editing. The exception is .NET: the SDK image only has its own runtime, so the tag follows
the highest `TargetFramework` in your projects (`dotnet/sdk:10.0` if none states one). When several
stacks are found the first one picks the image and the caches of all are listed.

## Rename a workspace

```sh
rooz move myworkspace newname [--dry-run]
```

Volumes cannot be renamed, so the work and data volumes are copied under the new name, the workspace
is rebuilt from its stored config (secrets need the same consent as `rooz update`) and the old one is
removed. The old workspace is stopped first; if anything fails, the partial new one is removed and
the old one is left in place. It never replaces: the new name must be free. Expect roughly twice the
workspace's disk use while it runs, install steps to run again, and shells and processes in the
workspace to end. The rebuilt workspace is left as `rooz new` leaves it; `rooz enter` it afterwards.

## Move a workspace to another host

```sh
rooz move myworkspace --to buildbox [newname] [--remove-source] [--dry-run]
```

The source is the engine you are on (`-H`/`DOCKER_HOST`); the target is a host from `rooz remote`.
Both must be connected. The volumes stream from one engine to the other through this machine (tar,
owners, modes and symlinks preserved, never held in memory whole), and the workspace is rebuilt on
the target from its config file with `rooz new` there, so that file must still be on this machine.
The source is left stopped; `--remove-source` removes it, otherwise `rooz rm` it when satisfied. If
anything fails, the partial workspace on the target is removed and the source is started again.
Not moved: caches, the ssh key (run `rooz system init` on the target and register its public key)
and images. A target on a different CPU architecture gets a warning: compiled artifacts will not run.

## Check the setup

```sh
rooz doctor
```

Checks that the engine is reachable and recent enough, that `rooz system init` was done (system
config and ssh key volumes, age identity on this machine) and, on docker, how much of the default
network address pool is used. Exits non-zero when something is missing.

## Manage caches

```sh
rooz cache ls [--all-scopes] [--json]   # size, age, mounting workspaces
rooz cache rm '~/.cargo'                # by path, or by name as shown in 'ls'
rooz cache prune [--dry-run]            # removes every cache no workspace mounts
```

`rm` and `prune` only touch your own scope (see `ROOZ_CACHE_SCOPE`) and refuse caches that a
container still mounts. Size shows `-` on engines without a disk usage report.

## Interactive shell in an anonymous ephemeral workspace

```sh
rooz tmp --image alpine --shell sh
```

