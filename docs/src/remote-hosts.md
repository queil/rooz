# Connecting to a remote Docker/Podman host

Rooz connects to a local Docker daemon by default. It can also work with any number of remote
hosts at the same time: `rooz remote` forwards each host's Docker/Podman socket over SSH to a
local unix socket, and `-H <host>` points a rooz command at one of them.

## Preparation

### The remote host

Rooz needs the engine socket path. Either give it to rooz when adding the host (`--socket`, below),
or export it at the top of `~/.bashrc` on the host (it is read by a non-interactive `ssh` command):

  * For Docker as root: `export DOCKER_HOST=/var/run/docker.sock`
  * For rootless Docker: `export DOCKER_HOST=/run/user/1000/docker.sock` (assuming your-user has uid=1000)
  * For Podman: `export DOCKER_HOST=/run/user/1000/podman/podman.sock` (assuming your-user has uid=1000)

### The local host

Define your hosts (stored in `~/.config/rooz/remotes.yaml`, or `$XDG_CONFIG_HOME/rooz/remotes.yaml`):

```sh
rooz remote add box1 ssh://your-user@box1
rooz remote add box2 ssh://your-user@box2 --socket /run/user/1000/podman/podman.sock
rooz remote ls
```

Then keep this running (e.g. in a spare terminal or tmux):

```sh
rooz remote          # connects to all configured hosts
rooz remote box1     # or just some of them
```

Each host's engine socket is forwarded to `$XDG_RUNTIME_DIR/rooz/<host>.sock` (`~/.rooz/<host>.sock`
if `XDG_RUNTIME_DIR` is not set). Hosts are independent: one being down or reconnecting does not
affect the others. Only one `rooz remote` can serve a given host at a time.

## Working with a host

```sh
rooz -H box1 new myws        # one-off
export ROOZ_HOST=box2        # per shell: every rooz command in it targets box2
rooz list                    # lists box2's workspaces
rooz list --all-hosts        # workspaces of every connected host
rooz -H box1 code myws       # attaches VsCode to the workspace on box1
```

Precedence: `-H` / `ROOZ_HOST`, then `DOCKER_HOST`, then the local engine. There is deliberately no
global "current host": use one terminal (or `ROOZ_HOST`) per host. `rooz system prune` and
`rooz remove --all` print the target they are about to hit.

Workspaces, volumes, SSH keys and caches live on each host's engine, so the same workspace name on
two hosts is two unrelated workspaces. The age identity and `ROOZ_TRUSTED_CONFIGS` are local and
apply to every host.

## Port forwarding

If any remote containers publish ports listed in their `forward-ports` label, `rooz remote` forwards
them to the same port on `127.0.0.1`. Local ports are shared by all hosts: if a port is already bound
by another process or forwarded for another host, it is skipped with a warning.

## VsCode

`rooz code` starts `code` with `DOCKER_HOST` set to the selected host's socket. If VsCode is already
running it may ignore that and use its own `docker.host` setting instead:

```json
   "docker.host": "unix:///run/user/1000/rooz/box1.sock"
```

