# VS Code

```sh
rooz code myws
```

Starts the workspace and opens VS Code attached to its work container at `/work`. Extensions,
terminals, debuggers and language servers run in the container; only the editor UI runs on your
machine.

Requirements:

* `code` on your `PATH`
* the [Dev Containers](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-containers)
  extension
* a local `docker` CLI. VS Code talks to the engine through it, so it must reach the same engine as rooz.

> [!NOTE]
> Some extensions ship native code built against glibc and don't work in Alpine-based images.

## Keep extensions across rebuilds

VS Code installs its server and extensions in `~/.vscode-server` inside the container. `rooz update`
recreates the container, so without a volume there every rebuild downloads them again. Mount a data
volume:

```yaml
data:
  vscode-server: {}
mounts:
  ~/.vscode-server: vscode-server
```

Data volumes belong to one workspace. Don't use a shared cache for this: one workspace's extensions
would run in every other.

## Per-workspace editor setup

Dev Containers keeps a configuration for attached containers, per image by default. Rooz names the work
container after the workspace, so a name-level configuration applies to one workspace. Attach with
`rooz code myws`, run **Dev Containers: Open Named Configuration File** from the Command Palette and add,
for example:

```json
{
  "extensions": ["rust-lang.rust-analyzer"],
  "settings": { "terminal.integrated.defaultProfile.linux": "bash" },
  "remoteEnv": { "RUST_LOG": "debug" },
  "postAttachCommand": "cargo fetch"
}
```

* VS Code applies it the first time it attaches to a container of that name, including the one
  `rooz update` recreates.
* The file lives in your local VS Code, not in the repo or the rooz config.
* To edit it while detached, use **Dev Containers: Open Attached Container Configuration File...**.
* `workspaceFolder` isn't needed: `rooz code` always opens `/work`.

The full list of properties is in VS Code's
[attached container configuration reference](https://code.visualstudio.com/docs/devcontainers/attach-container#_attached-container-configuration-reference).

## Remote hosts

```sh
rooz -H box1 code myws
```

`rooz code` starts `code` with `DOCKER_HOST` set to the host's socket. A VS Code that is already running
may ignore it and use its `docker.host` setting instead:

```json
"docker.host": "unix:///run/user/1000/rooz/box1.sock"
```

## Other editors

VS Code is the only integrated editor today. Terminal editors such as Neovim or Helix work inside
`rooz enter`; install them through the image or `install:` steps.
