# Secrets

Rooz encrypts sensitive values (e.g. API keys) in config files with
[age](https://github.com/C2SP/C2SP/blob/main/age.md).

## Example

Run `rooz system init` first.

Run `touch example.yaml` and `rooz config edit example.yaml`. Paste this into the editor, save and quit:

```yaml
secrets:
  apiKey: 1744420283158995

env:
  API_URL: https://api.example.com/v1
  API_KEY: "{{ apiKey }}"

```

`cat example.yaml` now shows the secret encrypted:

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

Inside the container, `API_KEY` holds the decrypted value:

```sh
rooz enter secrets-test

> echo $API_KEY
1744420283158995
```

## Where secrets may be used

Rooz expands secrets only when both conditions hold:

1. you allowed the secrets explicitly, and
2. the whole configuration comes from a trusted source.

Otherwise `rooz new` aborts and names the secrets involved. `vars` are not restricted.

**Allowing secrets:**

* `ROOZ_ALLOW_SECRETS=<names>` - comma-separated secret names, e.g. `ROOZ_ALLOW_SECRETS=dbPassword`.
  Unlisted secrets are still refused, so it is safe to keep in your shell profile. Also covers
  `rooz update`, which re-renders the configuration on every run.
* `ROOZ_ALLOW_SECRETS=true` - allows all secrets the configuration declares. Useful for CI.
* `--allow-secrets true` - same, for a single `rooz new`, `rooz update` or `rooz move`.

**Trusted configuration:** one of

* a local file passed with `--config`, with nothing else merged into it;
* a source listed in `ROOZ_TRUSTED_CONFIGS` (see [Trusted config sources](#trusted-config-sources)).

Rooz refuses to expand secrets if the workspace also merges:

* a `--config <url>//<path>` source not listed in `ROOZ_TRUSTED_CONFIGS`;
* a `bases` layer outside a trusted directory.

Any templated field (`env`, `install`, `command`, `shell`, `data` content, sidecar `env`) can reference
a secret, so whoever authors a merged layer could exfiltrate it, e.g. `env: {LEAK: "{{ MY_SECRET }}"}`.

⚠️ Rooz cannot tell your own `--config` file from one copied out of a repository. Never point
`--config` at a file from the repository you are opening, and read every base layer before including it.

### Trusted config sources

Ciphertext is not bound to a configuration: anyone who has the ciphertext can paste it into their own
config and rooz will decrypt it. Rooz therefore trusts configurations by source:

```sh
export ROOZ_TRUSTED_CONFIGS='git@github.com:me/rooz-configs,~/rooz-configs'
```

* Repository URL - `--config <url>//<path>` from it is trusted, including its `bases` (read from the
  same clone). Matched exactly as passed to `--config`: `.../configs` and `.../configs.git` are
  different entries.
* Local directory - a `--config` file is trusted, `bases` included, when the file and every base it
  pulls in resolve (symlinks followed) inside it.

A trusted source gets the same as your own local file: secrets (still subject to `ROOZ_ALLOW_SECRETS`)
and shared caches. Anyone who can push to a trusted repository can read your secrets - list only
repositories that are yours alone.

## Secrets at rest

Decrypted values are never written to the workspace-config volume. The container's environment and
generated files get the real values at creation time, but the persisted `runtime.config`
(`rooz config show <ws> --part runtime`) has every secret occurrence replaced with `***`. Config files
in that volume are owner-only.

ℹ️ `shell` is not masked, because `rooz enter` executes it. Do not reference secrets from `shell`.
