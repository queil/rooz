# Secrets

Rooz encrypts sensitive config values, such as API keys, with
[age](https://github.com/C2SP/C2SP/blob/main/age.md). The ciphertext is safe to commit.

## Example

After `rooz system init`, create a config and open it with `rooz config edit`:

```sh
touch example.yaml
rooz config edit example.yaml
```

Paste this, then save and quit:

```yaml
secrets:
  apiKey: 1744420283158995

env:
  API_URL: https://api.example.com/v1
  API_KEY: "{{ apiKey }}"
```

The file now holds the secret encrypted:

```yaml
secrets:
  apiKey: '-----BEGIN AGE ENCRYPTED FILE-----|YWdlLWVuY3J ... truncated'
env:
  API_URL: https://api.example.com/v1
  API_KEY: "{{ apiKey }}"
```

Create a workspace, allowing the secret (see [Where secrets may be used](#where-secrets-may-be-used)):

```sh
ROOZ_ALLOW_SECRETS=apiKey rooz new secrets-test --config ./example.yaml
```

Inside, `API_KEY` holds the decrypted value:

```sh
rooz enter secrets-test

> echo $API_KEY
1744420283158995
```

## Where secrets may be used

Rooz expands secrets only when both hold:

1. you allowed them explicitly, and
2. the whole configuration comes from a trusted source.

Otherwise `rooz new` stops and names the secrets involved. `vars` are not restricted.

**Allowing secrets**

* `ROOZ_ALLOW_SECRETS=<names>` allows the listed secrets (comma-separated), e.g.
  `ROOZ_ALLOW_SECRETS=dbPassword`. Others are still refused, so it is safe to keep in your shell
  profile. It also covers `rooz update`, which renders the configuration again on every run.
* `ROOZ_ALLOW_SECRETS=true` allows every secret the configuration declares. Useful in CI.
* `--allow-secrets true` does the same for a single `rooz new`, `rooz update` or `rooz move`.

**Trusted configuration** is either

* a local file passed with `--config`, with nothing else merged into it, or
* a source listed in `ROOZ_TRUSTED_CONFIGS` (see [Trusted config sources](#trusted-config-sources)).

Secrets are refused if the workspace also merges:

* a `--config <url>//<path>` source not listed in `ROOZ_TRUSTED_CONFIGS`, or
* a `bases` layer outside a trusted directory.

Any templated field (`env`, `install`, `command`, `shell`, `data` content, sidecar `env`) can reference
a secret, so whoever writes a merged layer could leak it, e.g. `env: {LEAK: "{{ MY_SECRET }}"}`.

> [!WARNING]
> Rooz cannot tell your own `--config` file from one copied out of a repository. Never point `--config`
> at a file from the repository you are opening, and read every base layer before including it.

### Trusted config sources

Ciphertext is not bound to a configuration: anyone who has it can paste it into their own config, and
rooz will decrypt it. So rooz trusts configurations by where they come from:

```sh
export ROOZ_TRUSTED_CONFIGS='git@github.com:me/rooz-configs,~/rooz-configs'
```

* **A repository URL** trusts `--config <url>//<path>` from that repository, including its `bases`,
  which are read from the same clone. URLs match exactly as passed to `--config`: `.../configs` and
  `.../configs.git` are different entries.
* **A local directory** trusts a `--config` file, `bases` included, when the file and every base it pulls
  in resolve inside it, symlinks followed.

A trusted source gets what your own local file gets: secrets (still subject to `ROOZ_ALLOW_SECRETS`) and
shared caches. Anyone who can push to a trusted repository can read your secrets, so list only
repositories that are yours alone.

## Secrets at rest

Decrypted values are never written to the workspace-config volume. The container's environment and
generated files get the real values at creation, but the stored `runtime.config`
(`rooz config show <ws> --part runtime`) has every secret replaced with `***`. Config files in that
volume are readable by their owner only.

> [!NOTE]
> `shell` is not masked, because `rooz enter` executes it. Do not reference secrets in `shell`.
