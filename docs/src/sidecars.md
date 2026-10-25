# Sidecars

Sidecars are containers that run next to the work container, a minimal docker-compose. They are defined
in the config file only:

```yaml
sidecars:
  sql:
    image: my:sql
    command:
    - --some
    env:
      TEST: "true"

  tools:
    image: my:tools
```

Sidecar names use lowercase letters, digits and `-`, starting with a letter or digit; `work` is
reserved. A sidecar's container is named `<workspace>_<sidecar>`.

A single config layer may omit `image`, e.g. an overlay extending a sidecar from `bases:`, but it must be
set once all layers are merged, or workspace creation fails.

`--container <name>` targets a sidecar instead of the work container in `rooz enter`, `rooz exec` and
`rooz logs`.

## Networking

Hub and spoke:

* The work container reaches every sidecar by name (`sql`, `tools` above) over a dedicated pair network.
  Container ID and IP work too.
* Every sidecar can reach the work container: pair networks are symmetric, so keep its listeners minimal.
* Sidecars reach each other only through `peers:`, or when both have `egress: true`.
* Sidecars have no internet access unless `egress: true`.
* Workspaces are isolated from each other.

`peers:` lists the sidecars a sidecar may reach:

* Each relation gets its own network, on which the two resolve each other by name.
* `a: peers [b]` is the same as `b: peers [a]`.
* Names must be sidecars of the same workspace.
* Across config layers, `peers` merges as a deduplicated union, unlike other lists, which are replaced:
  overlays can add edges, not remove them.

```yaml
sidecars:
  claude:
    peers: [proxy]   # no internet; reaches it only via the proxy
  proxy:
    egress: true     # internet access
  dkr:
    peers: [images]  # docker-in-docker pulling via the mirror
  images:
    egress: true     # registry pull-through cache, needs internet
```

> [!WARNING]
> Rooz creates one network per workspace, plus one per sidecar and one per peer relation. Docker's
> default address pools allow only about 31 networks per host. On an address pool error, set
> `default-address-pools` with a smaller subnet size (e.g. `"size": 24`) in `daemon.json`. Podman
> (netavark) is not affected.

## Egress policy

A sidecar running an agent can be limited to the hosts it needs, and kept from holding the
credentials it uses:

```yaml
sidecars:
  claude:
    egress:
      allow: [github.com, "*.githubusercontent.com", registry.npmjs.org]
      inject:
        - host: api.anthropic.com
          rules:
            - when_header: x-cc-req
              set: {authorization: "Bearer {{ secrets.anthropic_token }}"}
              remove: [x-cc-req]
```

* rooz adds a sidecar `<name>-egress` running joey's proxy (`joey egress-proxy`). It is the only one of
  the two with a way out; the sidecar reaches it, and nothing else outside the workspace, through
  their peer network. `HTTP(S)_PROXY` and `NO_PROXY` (itself and its peers) are set in the sidecar
  unless its `env` sets them.
* `allow`: hosts reached through as they are, TLS end to end. `*.example.com` is the hosts under
  `example.com`, not `example.com` itself. The proxy refuses any other host, and any port but `ports`
  (`[443]` unless set), before it is even resolved, and checks that a connection's TLS server name is
  the host it was opened for.
* `inject`: hosts the proxy intercepts to apply header rules: the first rule a request matches by
  `methods`, `paths` (a trailing `*` is a prefix) and `when_header` sets and removes headers. The
  values, typically from `secrets`, exist only in the proxy. The proxy presents certificates from a
  CA generated for the workspace: its certificate is mounted in the sidecar at
  `/usr/local/share/ca-certificates/rooz-egress.crt` and named by `NODE_EXTRA_CA_CERTS`; for tools
  that read the system store, add a `root: true` install step running `update-ca-certificates`. The
  CA is written once and kept across updates, and its key is never stored in the workspace's config.
* Plain HTTP is refused. Every decision is a line in `rooz logs <workspace> --container <name>-egress`.

## Keys

* `image`: container image.
* `env`: environment variables.
* `command`: overrides the entrypoint (`ENTRYPOINT`).
* `args`: overrides the entrypoint's arguments (`CMD`).
* `mounts`: container paths mapped to a `data` key or an inline value, as in the work container's
  `mounts`. See [Data and mounts](configuration.md#data-and-mounts). Volumes survive container restarts
  and removal:

  ```yaml
  data:
    sql-data: {}   # empty rw volume
  sidecars:
    sql:
      mounts:
        /var/opt/mssql: sql-data
        /etc/tool/config.json:
          content: |-
            {"some": "config"}
  ```

* `ports`: port bindings, in the same format as [work container ports](configuration.md#ports).
* `vpn` (bool): grants `NET_ADMIN`, `/dev/net/tun` and `src_valid_mark` for a VPN in the sidecar, with
  operator consent. See [VPN sidecars](configuration.md#vpn-sidecars).
* `peers`: sidecars this one may reach. See [Networking](#networking).
* `egress` (default `false`): `true` for outbound internet access - the sidecar joins the
  `<workspace>_egress` network, shared with the work container and other egress sidecars - or a policy
  that lets it out only through a proxy of its own. See [Egress policy](#egress-policy). Install steps
  always have internet access.
* `install`: install steps, built into an image. See [Sidecar images](configuration.md#sidecar-images).
* `uid`: uid to run as (default `0`).
* `shell`: shell for `rooz enter --container` (default `sh`).
* `init` (bool, default `true`): run an init process as pid 1.
* `privileged` (bool): with operator consent. See [Privileged containers](configuration.md#privileged-containers).
* `work_dir`: working directory.
