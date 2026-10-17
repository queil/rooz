# Variables and templating

* Syntax: [handlebars](https://handlebarsjs.com/guide/).
* Declare variables in `vars`, secrets in `secrets`. Secrets are decrypted before expansion.
* Works in most config values, not in YAML keys.
* Works inside `vars`, but only for variables defined earlier in the document.
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
