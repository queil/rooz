# Getting started

## Initialize

⚠️ Using a remote Docker host? [Configure it first](remote-hosts.md).

```sh
rooz system init
```

This creates:

* **An SSH key pair (ed25519)** for SSH auth (e.g. github.com). Stored in a volume and mounted
  read-only under `~/.ssh` in every work container. Workspaces can read it but not modify or
  delete it.

* **An age identity** for encrypting secrets in config. It is stored on your machine at
  `~/.config/rooz/age.key` (`$XDG_CONFIG_HOME/rooz/age.key` if set, or `ROOZ_AGE_KEY_FILE`), mode
  `0600`. It is never written to the engine (any engine API user can read all volumes) and is used
  only by the CLI.

  ℹ️ Back up this file. If it is lost, existing encrypted vars cannot be decrypted and must be
  re-encrypted. To use an existing identity, pass `--age-identity`. To use a second machine against
  the same engine, copy the identity file there.

  ℹ️ Upgrading: earlier versions kept the identity in the `rooz_sys-config` volume. The first rooz
  command after upgrading moves it to `age.engine.bak` and prints the path. It is not adopted
  automatically, as rooz cannot verify who wrote it. Check it, then install it:
  `rooz system init --force --age-identity "$(cat ~/.config/rooz/age.engine.bak)"`. If the engine is
  shared, treat the identity as compromised: re-key with `rooz system init --force` and re-encrypt
  your secrets.

`--force` regenerates the age identity, deleting the existing one. It does not touch the SSH key pair,
which existing workspaces mount. To replace the SSH key (e.g. if leaked):

```sh
rooz system init --rotate-ssh-key
```

This generates a new pair in place and prints the public key. Register it wherever you used the old
one and de-register the old one. Running workspaces pick it up on their next start.

⚠️ The SSH key is the engine's only git identity, and every workspace can read it. The workspace image
and user come from the repository's config, so any repository you open can take the key. Rotation
limits damage after the fact. Do not open repositories you would not trust with this key.

## Configure

ℹ️ See [Configuration](configuration.md).

Rooz works best with your own custom image(s). Image and user can be passed as CLI flags, but it is
easier to set them (and the shell) in `~/.bashrc` (or equivalent) on the host and override via CLI when needed.

### Example configuration

```sh
export ROOZ_USER=queil
export ROOZ_IMAGE=ghcr.io/queil/image:latest
export ROOZ_SHELL=bash
export ROOZ_CACHES='~/.local/share/containers/storage/'
```

## System config

`rooz system configure` sets the `.gitconfig` rooz uses for cloning (e.g. for aliases). That is its
only setting for now. The age identity is not part of it; it lives on your machine (see
[Initialize](#initialize)).
