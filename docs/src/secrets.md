# Secrets

Rooz uses [age](https://github.com/C2SP/C2SP/blob/main/age.md) encryption to safely store
sensitive data (like API keys) in config files.

## Example

Make sure your rooz has been initiated with `rooz system init`

Run `touch example.yaml` and `rooz config edit example.yaml`. Your default editor should open.

Paste the following config, save & quit.

```yaml
secrets:
  apiKey: 1744420283158995

env:
  API_URL: https://api.example.com/v1
  API_KEY: "{{ apiKey }}"

```

Now `cat example.yaml` and you should see something like the below:

```yaml
secrets:
  apiKey: '----BEGIN AGE ENCRYPTED FILE-----|YWdlLWVuY3J ... truncated'
env:
  API_URL: https://api.example.com/v1
  API_KEY: "{{ apiKey }}"

```

Now, we can create a new workspace like:

```sh
rooz new secrets-test --config ./example.yaml
```

And enter the container and the `API_KEY` var is value gets decrypted and injected:

```sh
rooz enter secrets-test

> echo $API_KEY
1744420283158995
```

## Where secrets may be used

Expanding a secret needs two things: a configuration that is yours, and your say-so.

**Your say-so.** A secret is expanded only into a configuration you have released it to. Without
that, `rooz new` aborts and names the secrets involved. Consent with either:

* `ROOZ_ALLOW_SECRETS=<names>` - a comma-separated list of the secrets you are releasing, e.g.
  `ROOZ_ALLOW_SECRETS=dbPassword`. Anything *not* named is still refused, which makes this safe to
  leave in your shell profile, and it also covers `rooz update` (which re-renders the configuration
  on every run).
* `ROOZ_ALLOW_SECRETS=true` - releases whatever the configuration declares. Convenient for CI.
* `--allow-secrets true` - the same, for a single `rooz new` or `rooz update`.

The reason is that a local `--config` path is not proof of authorship: a repository can ship a config
file and its README can tell you to pass it (`rooz new ws --config ./setup.yaml`), and that file then
picks the image, entrypoint, install steps and environment your plaintext lands in. Naming the
secrets keeps a standing consent from covering a file you did not write.

**A configuration that is yours.** Secrets are expanded only when the **whole** configuration is one
you wrote - a local file passed with `--config`, with nothing else merged into it, or a source you
listed as yours in `ROOZ_TRUSTED_CONFIGS`. Rooz refuses to expand secrets and fails with an explanation
if the workspace also merges:

* a `--config git:...` source you have not listed - it may be the repository's own configuration;
* a `bases` layer outside a trusted directory. A base layer is a separate file your config points at;
  it may be a team overlay or something copied out of a repository, and after the merge nothing can
  tell its fields apart from yours.

The reason: every templated field - `env`, `install`, `command`, `shell`, `data` content, sidecar
`env` - can be written by whoever authored that content. A layer shipping `env: {LEAK: "{{ MY_SECRET }}"}`
would otherwise receive your decrypted secret inside a container whose image and entrypoint it also
controls. `vars` keep working everywhere, since they are not sensitive.

⚠️ Consent is a gate, not a review. Rooz cannot tell your own `--config` file from one you
copied out of a repository you are opening, so releasing a secret to a configuration means you have
read it. Never point `--config` at a file that came from the repository, and read a base layer before
you include it.

### Trusted config sources

Encrypting a secret to your age identity does not tie it to your configuration: anyone who has seen the
ciphertext can paste it into a config of their own, and rooz would decrypt it for them. So rooz decides
what is yours by where the configuration comes from, and you tell it:

```sh
export ROOZ_TRUSTED_CONFIGS='git@github.com:me/rooz-configs,~/rooz-configs'
```

* a repository URL - a `--config <url>//<path>` from it counts as yours, including its `bases` (they are
  read from the same clone). Matched exactly as you pass it to `--config`: `.../configs` and
  `.../configs.git` are different entries. Never trusted over `http://` or `git://`, where anyone on the
  network path can substitute the content.
* a local directory - a `--config` file counts as yours, `bases` included, when the file and every base it
  pulls in resolve (symlinks followed) inside it.

A trusted source gets what your own local file gets: secrets (still subject to `ROOZ_ALLOW_SECRETS`)
and shared caches. Trusting a repository means trusting everyone who can push to it - list only
repositories that are yours alone.

## Secrets at rest

Decrypted secret values are never written to the workspace-config volume. The container's environment
and generated files are populated from the real values at creation time, but the persisted
`runtime.config` - the file `rooz config show <ws> --part runtime` prints - has every occurrence of a
secret replaced with `***`. Config files in that volume are written owner-only.

ℹ️ `shell` is the one field left unmasked, because `rooz enter` executes it. Do not
reference secrets from `shell`.

