<p align="center">
    <img width="180" src="https://github.com/queil/rooz/blob/main/assets/rooz512.webp?raw=true" alt="Rooz">
</p>

<h1 align="center">rooz</h1>

<p align="center"><b>Work in containers.</b> Docs: <a href="https://rooz.dev">rooz.dev</a></p>

Rooz runs your dev workspaces in containers, on a local or remote Docker/Podman engine. It has built-in
git cloning, SSH keys, encrypted secrets, shared caches, sidecars and declarative config.

## Install

```sh
curl -fsSL https://rooz.dev/install.sh | sh
```

Requires Docker >= 29.5.0 or Podman >= 6. Other options: [Install](https://rooz.dev/install.html).

## Quick start

```sh
rooz system init                                   # SSH and age keys
rooz new -g git@github.com:your/repo myworkspace   # create a workspace from a repo
rooz enter myworkspace                             # open a shell in it
```

Next: [Getting started](https://rooz.dev/getting-started.html) ·
[Configuration](https://rooz.dev/configuration.html) ·
[CLI reference](https://rooz.dev/reference/cli.html)
