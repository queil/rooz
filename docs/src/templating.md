# Variables/templating

Rooz supports basic variable replacement/templating:
* [handlebars](https://handlebarsjs.com/guide/) syntax is used
* variables are declared via `vars`
* secrets are declared via `secrets`
* secrets are decrypted before expanding
* handlebars can be used in the most of the config values but not in YAML keys
* vars/secrets replacement works within `vars` themselves too. However, only if the var usage is 
below the var definition (in the document order).
* the secret section does not support var/secrets replacement

```yaml
secrets:
  sqlPassword: '----BEGIN AGE ENCRYPTED FILE-----|YWdlLWVuY3J ... truncated'

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

