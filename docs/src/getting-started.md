# Getting started

## Initialize

⚠️ If you intend to use a remote Docker host with rooz please [configure it first](remote-hosts.md)

To initialize rooz run the following command:

```sh
rooz system init
```

The command creates:

* an SSH key pair (ed25519) intended to use for auth wherever SSH keys can be used (like github.com)
  
  The generated key gets stored in a volume and then mounted **read-only** under `~/.ssh` to all rooz
  workspace containers, so a workspace cannot overwrite or delete it. Note that workspace containers
  can still *read* the key - that is the point of mounting it - and the workspace image and user come
  from the repository's configuration. Do not open repositories you would not trust with that key.

* an age encryption key pair intended to use for encryption of sensitive config data (i.e. secrets)

  The identity is stored **on your machine**, at `~/.config/rooz/age.key` (`$XDG_CONFIG_HOME/rooz/age.key`
  if set, or wherever `ROOZ_AGE_KEY_FILE` points), mode `0600`. It is never written to the engine: a
  container engine hands the contents of every named volume to every one of its API users, and this key
  decrypts every secret you have ever encrypted with rooz - including ciphertext you commit to a
  repository. Only the CLI uses it; nothing inside a container ever needs it.

  ℹ️ It's important to back up the generated age identity - copy that file somewhere safe.
  If the key is lost all the existing config files with encrypted vars won't decrypt and re-encrypting will be required.
  To init rooz with an existing age identity use the `--age-identity` switch. Working from a second machine
  against the same engine means copying the identity file there.

  ℹ️ Upgrading: earlier versions kept the identity in the `rooz_sys-config` volume. The first
  rooz command after upgrading takes it out of the volume and keeps it aside at `age.engine.bak`, printing
  where it went - it is **not** adopted as this machine's identity, because rooz cannot tell your own
  upgraded key from one somebody else with engine access wrote there, and using theirs would encrypt
  every secret you save afterwards to their recipient. Check the file, then install it yourself:
  `rooz system init --force --age-identity "$(cat ~/.config/rooz/age.engine.bak)"`. If that engine is
  shared with anyone, treat the identity as compromised instead: re-key with `rooz system init --force`
  and re-encrypt your secrets.

You can regenerate the age identity by specifying the `--force` parameter. Please note that the existing
age key will be wiped out.

The SSH key pair is deliberately kept across re-inits - every existing workspace mounts it - so `--force`
does not touch it. To replace it (the remediation if you believe it leaked) run:

```sh
rooz system init --rotate-ssh-key
```

That generates a new pair in place and prints the new public key. Register it wherever you used the old
one and de-register the old one; running workspaces pick the new key up on their next start.

⚠️ The ssh key is the engine's single git identity and every workspace can read it (see above), so
a repository you open can take it. Rotation limits the damage after the fact; it does not prevent the
theft. Treat opening an untrusted repository as handing over that key.

## Configure

ℹ️ Read more in the [Configuration](configuration.md) section

Rooz works best with user's provided custom image(s).
You can pass image, shell, and user via cli parameters but it's much more convenient to set it up 
in your host's `~/.bashrc` (or a non-bash equivalent), and only override it via cli when required.

### Example configuration:

```sh
export ROOZ_USER=queil
export ROOZ_IMAGE=ghcr.io/queil/image:latest
export ROOZ_SHELL=bash
export ROOZ_CACHES='~/.local/share/containers/storage/'
```

## System config

Rooz exposes some system config via `rooz system configure`. At present it only allows to specify `.gitconfig` used by rooz for cloning. It can be used e.g. to specify aliases. This feature is new and may be extended in the future. The age identity is *not* part of it - that lives on your machine (see [Initialize](#initialize)).

