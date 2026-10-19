# Remote hosts

Rooz uses the local engine by default, and can work with several remote hosts at once: `rooz remote`
forwards each host's Docker/Podman socket over SSH to a local unix socket, and `-H <host>` targets one
of them.

## Setup

### On the remote host

Rooz needs the engine's socket path. Pass it with `--socket` when adding the host (below), or export it
at the top of the host's `~/.bashrc`, which non-interactive `ssh` reads:

* Docker as root: `export DOCKER_HOST=/var/run/docker.sock`
* Rootless Docker: `export DOCKER_HOST=/run/user/1000/docker.sock` (uid 1000)
* Podman: `export DOCKER_HOST=/run/user/1000/podman/podman.sock` (uid 1000)

### On your machine

Add the hosts. They are stored in `~/.config/rooz/remotes.yaml` (`$XDG_CONFIG_HOME/rooz/remotes.yaml` if
set, or `ROOZ_REMOTES_FILE`):

```sh
rooz remote add box1 ssh://your-user@box1
rooz remote add box2 ssh://your-user@box2 --socket /run/user/1000/podman/podman.sock
rooz remote ls
```

Then keep `rooz remote` running, e.g. in tmux:

```sh
rooz remote          # connects to every configured host
rooz remote box1     # or only some
```

* Each socket is forwarded to `$XDG_RUNTIME_DIR/rooz/<host>.sock` (`~/.rooz/<host>.sock` if
  `XDG_RUNTIME_DIR` is unset).
* Hosts are independent: one going down or reconnecting does not affect the others.
* Only one `rooz remote` can serve a given host at a time.

## Working with a host

```sh
rooz -H box1 new myws        # one command
export ROOZ_HOST=box2        # this shell: every rooz command targets box2
rooz list                    # box2's workspaces
rooz list --all-hosts        # workspaces on every connected host
rooz -H box1 code myws       # VS Code attached to the workspace on box1
```

* Precedence: `-H` / `ROOZ_HOST`, then `DOCKER_HOST`, then the local engine.
* There is no global current host: use one terminal, or one `ROOZ_HOST`, per host.
* `rooz system prune` and `rooz rm --all` print their target first.
* Workspaces, volumes, SSH keys and caches live on each host's engine. The same workspace name on two
  hosts means two unrelated workspaces.
* The age identity and `ROOZ_TRUSTED_CONFIGS` are local and apply to all hosts.

## Port forwarding

`rooz remote` forwards the ports in a remote container's `dev.rooz.forward-ports` label (set from
`ports`) to the same port on `127.0.0.1`. Local ports are shared across hosts: a port already bound
locally, or forwarded for another host, is skipped with a warning.

## VS Code

See [VS Code: remote hosts](vscode.md#remote-hosts).
