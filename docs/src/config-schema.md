# Config schema

Each release publishes the config file's JSON Schema at [rooz.dev/schema.json](https://rooz.dev/schema.json).
Use it with YAML language servers via a modeline:

```yaml
# yaml-language-server: $schema=https://rooz.dev/schema.json
```

It matches the latest release. For the schema of your installed version:

```sh
rooz config schema > rooz.schema.json
```
