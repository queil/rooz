# Variables and templating

Config values are [handlebars](https://handlebarsjs.com/guide/) templates. Declare variables under
`vars` and secrets under `secrets`; secrets are decrypted before expansion.

* Works in most values, not in YAML keys.
* Works inside `vars`, for variables defined earlier in the document.
* Does not work inside `secrets`.

```yaml
secrets:
  sqlPassword: '-----BEGIN AGE ENCRYPTED FILE-----|YWdlLWVuY3J ... truncated'

vars:
  sqlUser: admin
  sqlConnectionString: "uid={{ sqlUser }};pwd={{ sqlPassword }}"

env:
  SQL_CONNECTION_A: "database=A;{{ sqlConnectionString }}"
  SQL_CONNECTION_B: "database=B;{{ sqlConnectionString }}"

sidecars:
  tool:
    env:
      SQL_USER: "{{ sqlUser }}"
      SQL_PASSWORD: "{{ sqlPassword }}"
```
