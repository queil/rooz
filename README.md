# rooz

Trick yourself to feel home in a container.
Let's make the host just have your browser, your VS Code, and your Docker.
Let's do everything else in containers. Store everything in Docker volumes.
Let's bring your own Docker image with tooling & config.
Let's see what happens...

## Limitations

* Experiment/POC
* So far `linux-amd64` only

## Install

Assuming `~/.local/bin` exists and you have it in the `PATH`:

```sh
curl -sSL https://github.com/queil/rooz/releases/download/v0.2.0/rooz -o ~/.local/bin/rooz && chmod +x ~/.local/bin/rooz
```

## Usage

### Defaults

Rooz uses the following default config:

* `ROOZ_IMAGE=alpine/git:latest`
* `ROOZ_SHELL=sh`
* `ROOZ_USER=root`

This is really not recommended from both the user experience and security point of view. For the best UX please follow the below steps:

1. First bring your own image by adding something similar to your `.bashrc`:
```
export ROOZ_IMAGE=ghcr.io/queil/image:0.10.0
```

Example: [my own image](https://github.com/queil/image/blob/main/src/Dockerfile)

Rooz by default runs containers as `root` - not recommended. It's the best to create a user and set it with `USER` command in you Docker image.

2. Init `rooz` - it generates a new ssh key, stores it in a Docker volume, later auto-mounted to your work containers:

```sh
rooz init
```

Before moving on make sure you add your newly generated public key to your git provider.

3. Runs a container, cloning a repo:

```sh
rooz git@github.com:docker/awesome-compose.git
```

## Tricks

* You can also run a container without cloning with just typing `rooz`.
* You can run scratchpad container by setting the `--temp` flag. Once the container terminates it's all gone.
* You can use docker as if you were on the host - just include the docker cli with the build and compose plugins in your image.
  `rooz` auto-mounts the host's Docker sock into all the containers it launches. P.S. this is not DinD, this is DooD (Docker outside of Docker)
* You can install `rooz` in your image and then launch `rooz` in containers *ad infinitum*
