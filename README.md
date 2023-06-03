# rooz

:warning: This project in the current state is experimental. DO NOT USE.

TLDR: Rooz is a tool similar to [toolbox](https://docs.fedoraproject.org/en-US/fedora-silverblue/toolbox/)
or [distrobox](https://github.com/89luca89/distrobox) but unlike them it aims to share as little as possible with the host. Rooz is developed with software development in mind so it has a built-in support for git repositories.

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
rooz tmp --rm --image alpine --shell sh
```

## Configuration

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

* `rooz` runs as uid `1000` (always - it's hard-coded) so make sure it exists in your image
(with `rooz_user` as the name - it can be overridden via `ROOZ_USER` or `--user`)

### Shell

* the default shell is `bash` but you can override it via:
    * `ROOZ_SHELL` env var
    * `--shell` cmd-line parameter
    * in `.rooz.toml` via `shell`

### Caching

* caching - `rooz` supports basic path-keyed shared caches. It can be set per-repo like:

    ```toml
    caches = [
      "~/.nuget"
    ]
    ```

    All the repos specifying a cache path will share a container volume mounted at that path enabling cache reuse.
    It also can be set globally via `ROOZ_CACHES` (comma-separated paths). The global paths get combined with repo-specific paths.

## Other facts

* cloned git repos are mounted under `~/work/{repo_name}` where `repo_name` is the default one generated by `git` during cloning.
* you can enable `rooz` debug logging by setting the `RUST_LOG=rooz` env variable

* if `rooz` misbehaves you can go nuclear and run `rooz system prune` to remove all the running rooz containers and volumes excluding the ssh key volume. If you want to delete it do: `docker volume rm --force rooz-ssh-key-vol`

  :warning: `rooz system prune` deletes all your state held with `rooz` so make sure everything important is stored before.

## Limitations

* Experiment/POC so may contains traces of bugs, (or even some whole bugs)
* So far `linux-amd64` only
* running in WSL2 in Docker Desktop/Rancher Desktop(Moby) has permissions issues all the volumes get mounted as root

## Known issues

* When a volume is first crated container automatically populates it from the image
  (assuming there are any files in the corresponding directory in the image). It only happens if
  the volume is empty. So if you try to mount pre-existing volumes to a container with a new image the volumes' contents won't be updated.
  It is particularly annoying with the home dir volume as it holds user-specific configurations and may be wildly different from
  one image to another. A workaround could be to drop the home dir volume so that it gets recreated with the new content, however
  that way we lose things like `.bash_history`. To be resolved...

* auto-resizing rooz session to fit the terminal window (if resized) is not implemented

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

* [my image I use with rooz](https://github.com/queil/image/blob/main/src/Dockerfile)
