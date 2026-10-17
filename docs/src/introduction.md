# rooz

<p align="center"><img width="180" src="rooz512.webp" alt="Rooz"></p>

Rooz is a CLI tool that enables you to work in containers. It is intended for developers and DevOps engineers. Because of that, it comes with a built-in support for git repositories, SSH keys generation, support for git-safe secrets, shared caches, sidecar containers, declarative configuration, and a robust CLI. Rooz is designed to share as little as possible with the host so it can also orchestrate your workspaces on remote Docker/Podman hosts.

## Requirements

Rooz requires one of the following container backends:
* Docker >= 29.5.0
* Podman >= 6
* Rancher Desktop - untested atm

The engine is assumed to be yours - a rootless daemon on your own machine, or a remote one only you
use. Rooz defends your host, your other workspaces and your secrets against the repositories you
open; it cannot defend a workspace against another tenant of the same engine. Anyone who can reach
the engine API can read every volume, run containers on your networks, and name images and volumes
whatever they like, so sharing one engine between people who do not trust each other is out of scope.

## Resources

* [my image I use with rooz](https://github.com/queil/image/blob/main/src/Containerfile)
