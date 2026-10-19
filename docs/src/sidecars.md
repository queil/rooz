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
* `egress` (bool, default `false`): outbound internet access. Joins the `<workspace>-egress` network,
  shared with the work container and other egress sidecars. Install steps always have internet access.
* `install`: install steps, built into an image. See [Sidecar images](configuration.md#sidecar-images).
* `uid`: uid to run as (default `0`).
* `shell`: shell for `rooz enter --container` (default `sh`).
* `init` (bool, default `true`): run an init process as pid 1.
* `privileged` (bool): with operator consent. See [Privileged containers](configuration.md#privileged-containers).
* `work_dir`: working directory.
