# Config schema

The JSON Schema of the config file, for YAML language servers, is published with every release at
[rooz.dev/schema.json](https://rooz.dev/schema.json). Point your editor at it with a modeline:

```yaml
# yaml-language-server: $schema=https://rooz.dev/schema.json
```

It describes the latest release. To match the rooz you have installed, write the schema locally
instead:

```sh
rooz config schema > rooz.schema.json
```
