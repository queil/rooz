# rooz

## Trick yourself into feeling home in a container.

* Let's make the host just have your browser, your VS Code, and your Docker.
* Let's do everything else in containers; store everything in Docker volumes.
* Let's bring your own Docker image with tooling & config.
* Let's see what happens...

## Basic facts

* The first time you run `rooz` it generates you an SSH key pair and stores it in a Docker volume.
  Use that key to authenticate to your repos. They ssh key volume is then shared between all `rooz` containers.

* If you just run `rooz` it launches a free-style container, i.e. doesn't clone anything.
* You can also clone git repos with `rooz` and they'll run either in:
    * the default image: `alpine/git:latest`
    * an image you specified via `ROOZ_IMAGE`
    * an image you specified via `--image` cmd-line parameter
    * an image specified in the cloned repo in `.rooz.toml` like:

    ```toml
    image = "ghcr.io/queil/image:0.16.0-dotnet"
    shell = "bash"
    ```

* `rooz` runs as uid `1000` (always - it's hard-coded) so either make sure it exists in your image or rooz will attempt to auto-create it (with `rooz_user` as the name)
* the default shell is `sh` but you can override it via:
    * `ROOZ_SHELL` env var
    * `--shell` cmd-line parameter
    * in `.rooz.toml` via `shell`

* caching - `rooz` supports basic path-keyed shared caches. It is only supported via `.rooz.toml`:

    ```toml
    caches = [
      "~/.nuget"
    ]
    ```

    All the repos specyfing a cache path will share a Docker volume mounted at that path enabling cache reuse.

* if `rooz` misbehaves you can go nuclear and run `rooz --prune` to remove all the running rooz containers and volumes.

  :warning: `rooz --prune` deletes all your state held with `rooz` so make sure everything important is stored before.

## Tips & tricks:

* You can use docker as if you were on the host - just include the docker cli with the build and compose plugins in your image.
`rooz` auto-mounts the host's Docker sock into all the containers it launches. P.S. this is not DinD, this is DooD (Docker outside of Docker)
* You can install `rooz` in your image and then launch `rooz` in containers *ad infinitum*

## Limitations

* Experiment/POC
* So far `linux-amd64` only
* This is my first Rust project (learning the language) so please forgive the code quality here

## Install

```sh
curl -sSL https://github.com/queil/rooz/releases/download/v0.5.0/rooz -o ./rooz && chmod +x ./rooz && sudo mv ./rooz /usr/local/bin
```
