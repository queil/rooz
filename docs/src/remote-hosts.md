# Connecting to a remote Docker/Podman host

Rooz uses the local Docker daemon by default. It can work with multiple remote hosts at once: `rooz remote` forwards each host's Docker/Podman socket over SSH to a local unix socket, and `-H <host>` targets one of them.

## Preparation

### The remote host

Rooz needs the engine socket path. Pass it with `--socket` when adding the host (below), or export it at the top of `~/.bashrc` on the host (read by non-interactive `ssh`):

  * Docker as root: `export DOCKER_HOST=/var/run/docker.sock`
  * rootless Docker: `export DOCKER_HOST=/run/user/1000/docker.sock` (uid 1000)
  * Podman: `export DOCKER_HOST=/run/user/1000/podman/podman.sock` (uid 1000)

### The local host

Define hosts (stored in `~/.config/rooz/remotes.yaml`, `$XDG_CONFIG_HOME/rooz/remotes.yaml` if set, or `ROOZ_REMOTES_FILE`):

```sh
rooz remote add box1 ssh://your-user@box1
rooz remote add box2 ssh://your-user@box2 --socket /run/user/1000/podman/podman.sock
rooz remote ls
```

Keep this running (e.g. in tmux):

```sh
rooz remote          # connects to all configured hosts
rooz remote box1     # or just some of them
```

* each socket is forwarded to `$XDG_RUNTIME_DIR/rooz/<host>.sock` (`~/.rooz/<host>.sock` if `XDG_RUNTIME_DIR` is unset)
* hosts are independent: one down or reconnecting doesn't affect the others
* only one `rooz remote` can serve a given host at a time

## Working with a host

```sh
rooz -H box1 new myws        # one-off
export ROOZ_HOST=box2        # per shell: every rooz command in it targets box2
rooz list                    # lists box2's workspaces
rooz list --all-hosts        # workspaces of every connected host
rooz -H box1 code myws       # attaches VsCode to the workspace on box1
```

* precedence: `-H` / `ROOZ_HOST`, then `DOCKER_HOST`, then the local engine
* there is no global "current host": use one terminal (or `ROOZ_HOST`) per host
* `rooz system prune` and `rooz rm --all` print their target first
* workspaces, volumes, SSH keys and caches live on each host's engine: the same workspace name on two hosts means two unrelated workspaces
* the age identity and `ROOZ_TRUSTED_CONFIGS` are local and apply to all hosts

## Port forwarding

`rooz remote` forwards ports listed in a remote container's `dev.rooz.forward-ports` label (set from `ports`) to the same port on `127.0.0.1`. Local ports are shared by all hosts: a port already bound locally or forwarded for another host is skipped with a warning.

## VsCode

`rooz code` starts `code` with `DOCKER_HOST` set to the host's socket. An already-running VsCode may ignore it and use its `docker.host` setting instead:

```json
   "docker.host": "unix:///run/user/1000/rooz/box1.sock"
```
