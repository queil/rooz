# Getting started

## Initialize

> [!NOTE]
> On a remote Docker/Podman host? [Connect to it](remote-hosts.md) first.

```sh
rooz system init
```

This creates two keys:

* **An SSH key pair (ed25519)** for git and other SSH auth. It lives in a volume, mounted read-only at
  `~/.ssh` in every work container: workspaces can read it, not change or delete it.
* **An age identity** for encrypting secrets in config. It stays on your machine, at
  `~/.config/rooz/age.key` (`$XDG_CONFIG_HOME/rooz/age.key` if set, or `ROOZ_AGE_KEY_FILE`), mode
  `0600`. Only the CLI uses it; it is never written to the engine, where any API user can read every
  volume.

> [!IMPORTANT]
> Back up the age identity. Without it, encrypted values cannot be decrypted and must be encrypted
> again. Pass `--age-identity` to use an existing one; copy the file to use a second machine with the
> same engine.

`--force` replaces the age identity and deletes the old one. It leaves the SSH key alone, since existing
workspaces mount it. To replace the SSH key, e.g. after a leak:

```sh
rooz system init --rotate-ssh-key
```

This replaces the key pair in place and prints the new public key. Register it wherever the old one was
registered, then remove the old one. Running workspaces pick up the new key on their next start.

> [!WARNING]
> The SSH key is the engine's only git identity, and every workspace can read it. A config
> chooses the workspace image and user, so a config from a source you do not control can take the key. Rotation limits
> the damage afterwards; it does not prevent it. Only open repositories you would trust with this key.

## Configure

Rooz works best with an image of your own. Image and user can be passed as flags, but it is simpler to
set them, and the shell, in your shell profile and override with flags when needed:

```sh
export ROOZ_USER=queil
export ROOZ_IMAGE=ghcr.io/queil/image:latest
export ROOZ_SHELL=bash
export ROOZ_CACHES='~/.local/share/containers/storage/'
```

All options: [Configuration](configuration.md).

## Git config for cloning

`rooz system configure` opens the system config in `$EDITOR`. It holds the `.gitconfig` rooz clones
with, e.g. for aliases.
