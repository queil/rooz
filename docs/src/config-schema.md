# Config schema

Each release publishes the config file's JSON Schema at [rooz.dev/schema.json](https://rooz.dev/schema.json).
Point a YAML language server at it with a modeline:

```yaml
# yaml-language-server: $schema=https://rooz.dev/schema.json
```

It tracks the latest release. For the schema of the version you have installed:

```sh
rooz config schema > rooz.schema.json
```
