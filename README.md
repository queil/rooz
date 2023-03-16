# rooz

## Trick yourself into feeling home in a container.

* Let's make the host just have your browser, your VS Code, and your Docker.
* Let's do everything else in containers; store everything in Docker volumes.
* Let's bring your own Docker image with tooling & config.
* Let's see what happens...

## Basic facts

* :warning: This project in the current state is unsecure and experimental. DO NOT USE.  

* The first time you run `rooz` it generates you an SSH key pair and stores it in a Docker volume.
  Use that key to authenticate to your repos. They ssh key volume is then shared between all `rooz` containers.

* If you just run `rooz` it launches a free-style container, i.e. doesn't clone anything.
* You can also clone git repos with `rooz` and they'll run either in:
    * the default image: `bitnami/git:latest`
    * an image you specified via `ROOZ_IMAGE`
    * an image you specified via `--image` cmd-line parameter
    * an image specified in the cloned repo in `.rooz.toml` like:

    ```toml
    image = "ghcr.io/queil/image:0.16.0-dotnet"
    shell = "bash"
    ```

* `rooz` runs as uid `1000` (always - it's hard-coded) so either make sure it exists in your image or rooz will attempt to auto-create it
(with `rooz_user` as the name - it can be overridden via `ROOZ_USER` or `--user`)
* the default shell is `bash` but you can override it via:
    * `ROOZ_SHELL` env var
    * `--shell` cmd-line parameter
    * in `.rooz.toml` via `shell`

* cloned git repos are always mounted under `~/work/{repo_name}` where `repo_name` is the default one generated by `git` during cloning.
* caching - `rooz` supports basic path-keyed shared caches. It is only supported via `.rooz.toml`:

    ```toml
    caches = [
      "~/.nuget"
    ]
    ```

    All the repos specyfing a cache path will share a Docker volume mounted at that path enabling cache reuse.

* you enable `rooz` debug logging by setting `RUST_LOG=rooz` env variable

* if `rooz` misbehaves you can go nuclear and run `rooz --prune` to remove all the running rooz containers and volumes excluding
  the ssh key volume. If you want to delete it do: `docker volume rm --force rooz-ssh-key-vol`

  :warning: `rooz --prune` deletes all your state held with `rooz` so make sure everything important is stored before.

## Tips & tricks:

* You can use docker as if you were on the host - just include the docker cli with the build and compose plugins in your image.
`rooz` auto-mounts the host's Docker sock into all the containers it launches. P.S. this is not DinD, this is DooD (Docker outside of Docker)
* You can install `rooz` in your image and then launch `rooz` in containers *ad infinitum*

## Limitations

* Experiment/POC so may contains traces of bugs, (or even some whole bugs)
* So far `linux-amd64` only
* This is my first Rust project (learning the language) so please excuse my the code quality here
* rooz's `known_hosts` only contains github.com entries
* Only tested with some alpine and ubuntu images. It may work with other distros too.

## Install

```sh
curl -sSL https://github.com/queil/rooz/releases/latest/download/rooz -o ./rooz && chmod +x ./rooz && sudo mv ./rooz /usr/local/bin
```

## Known issues

* When a volume is first crated Docker automatically populates it from the image
  (assuming there are any files in the corresponding directory in the image). It only happens if
  the volume is empty. So if you try to mount pre-existing volumes to a container with a new image the volumes' contents won't be updated.
  It is particularly annoying with the home dir volume as it holds user-specific configurations and may be wildly different from
  one image to another. A workaround could be to drop the home dir volume so that it gets recreated with the new content, however
  that way we lose things like `.bash_history`. To be resolved...

## Running with Podman

:warning: This feature barely works and may be further explored (or not)

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

## Resources

* [my image I use with rooz](https://github.com/queil/image/blob/main/src/Dockerfile)
