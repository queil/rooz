# rooz

Rooz is a CLI tool that enables you to work in containers. It is intended for developers and DevOps engineers. Because of that, it comes with a built-in support for git repositories, SSH keys generation, shared caches, and a robust CLI. Rooz is similar to [toolbox](https://docs.fedoraproject.org/en-US/fedora-silverblue/toolbox/)
and [distrobox](https://github.com/89luca89/distrobox) but unlike them it aims to share as little as possible with the host.

:warning: This project in the current state is experimental. Use at your own risk.

## Quick start

:warning: The binary is only available for `linux_amd64` at the moment.

### Install

```sh
curl -sSL https://github.com/queil/rooz/releases/latest/download/rooz -o ./rooz && chmod +x ./rooz && sudo mv ./rooz /usr/local/bin
```
### Initialize

The below command creates an SSH key pair (ed25519). You can use it to authenticate wherever SSH keys can be used (like github.com):

```sh
rooz system init
```
The generated key gets stored in a volume and then mounted under `~/.ssh` to all rooz containers.

💡 You can regenerate the key by specifying the `--force` parameter. Please note that the existing key will be wiped out.

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

## Usage examples

### Create an empty workspace

```sh
rooz new myworkspace
```

### Create a workspace from a git repo

```sh
rooz new -g git@github.com:your/repo.git myworkspace2
```
### 

### Enter a previously created workspace

```sh
rooz enter myworkspace2
```

### Interactive shell in an anonymous ephemeral workspace

```sh
rooz tmp --image alpine --shell sh
```

## Configuration

Most of the settings can be configured via:

* environment variables
* `.rooz.toml` in the cloned repository (if any)
* `.rooz.toml` file specified via `--config` (on `rooz new`)
* cmd-line parameters

The configuration file (`.rooz.toml`) provides the most options: [example](examples/dotnet-nats.rooz.toml)

### Images

:information_source: the default image is `docker.io/bitnami/git:latest`

There are a few ways of specifying images:
* via the `ROOZ_IMAGE` env variable
* via  the `--image` cmd-line parameter
* if creating a workspace with a git repository via `.rooz.toml` in the root of that repository:

  ```toml
  image = "ghcr.io/queil/image:dotnet"
  shell = "bash"
  ```

### User

`rooz` runs as uid `1000` (always - it's hard-coded) so make sure it exists in your image
(with `rooz_user` as the name - it can be overridden via `ROOZ_USER` or `--user`)

### Shell

The default shell is `bash` but you can override it via:

* `ROOZ_SHELL` env var
* `--shell` cmd-line parameter (on `rooz enter`)
* in `.rooz.toml` via `shell`

### Caching

`rooz` supports basic path-keyed shared caches. It can be set per-repo like:

```toml
caches = [
  "~/.nuget"
]
```

All the repos specifying a cache path will share a container volume mounted at that path enabling cache reuse.
It also can be set globally via `ROOZ_CACHES` (comma-separated paths). The global paths get combined with repo-specific paths.

### Port mappings

Port mappings for the work container can be specified via `.rooz.toml` only:

```toml
ports = [
  "80:8080",
  "22:8022"
]
```

## Sidecars

*It's similar to docker-compose but super simple and limited to bare minimum.*

* `rooz` has a limited and experimental support for sidecars (containers running along). It is only available via `.rooz.toml`:

  ```toml
  [sidecars.sql]
  image = "my:sql"
  command = ["--some"]

  [sidecars.sql.env]
  TEST="true"

  [sidecars.tools]
  image = "my:tools"
  
  ```
  All containers within a workspace are connected to a workspace-wide network. They can *talk* to each other using sidecar names. In the above examples that would be `sql` and `tools`. Also the usual container ID and IP works too, but it is not as convenient.

* the `enter` command now lets you specify `--container` to enter (otherwise it enters the work container).

Supported keywords:
* `image` - set containers image
* `env` - set environment variables
* `command` - override container command
* `mounts` - mount automatically-named rw volumes at the specified paths (so they can survive container restarts/deletes).

## Other facts

* cloned git repos are mounted under `~/work/{repo_name}` where `repo_name` is the default one generated by `git` during cloning.
* you can enable `rooz` debug logging by setting the `RUST_LOG=rooz` env variable

* if `rooz` misbehaves you can go nuclear and run `rooz system prune` to remove ALL the rooz containers and volumes. You can also remove just the workspaces, (leaving shared caches volumes, and the ssh volume untouched), by: `rooz rm --all --force`

  :warning: `rooz system prune` deletes all your state held with `rooz` so make sure everything important is stored before.

## Known issues

* When a volume is first crated container automatically populates it from the image
  (assuming there are any files in the corresponding directory in the image). It only happens if
  the volume is empty. So if you try to mount pre-existing volumes to a container with a new image the volumes' contents won't be updated.
  It is particularly annoying with the home dir volume as it holds user-specific configurations and may be wildly different from
  one image to another. A workaround could be to drop the home dir volume so that it gets recreated with the new content, however
  that way we lose things like `.bash_history`. To be resolved...

* auto-resizing rooz session to fit the terminal window (if resized) is not implemented. Workaround: exit the container, resize the window to your liking, enter the container.

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
