# rooz

<p align="center"><img width="180" src="rooz512.webp" alt="Rooz"></p>

Rooz is a CLI for working in containers, aimed at developers and DevOps engineers. Features: git repositories, SSH key generation, git-safe secrets, shared caches, sidecar containers, declarative configuration. It shares as little as possible with the host, so it can also run workspaces on remote Docker/Podman hosts.

## Requirements

One of:
* Docker >= 29.5.0
* Podman >= 6
* Rancher Desktop - untested

The engine must be yours: a rootless daemon on your machine, or a remote one only you use. Rooz protects your host, other workspaces and secrets from the repositories you open, but not from other users of the same engine - anyone with engine API access can read every volume, run containers on your networks and spoof image/volume names. Don't share an engine with untrusted users.
