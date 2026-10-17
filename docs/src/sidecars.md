# Sidecars

Sidecars are containers running alongside the work container - a minimal docker-compose. Define them in the config file only:

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

`image` may be omitted in a single config layer (e.g. an overlay extending a sidecar from `bases:`), but must be set once all layers are merged, or workspace creation fails.

`--container <name>` targets a sidecar instead of the work container in `rooz enter`, `rooz exec` and `rooz logs`.

## Networking

Hub-and-spoke:

* the work container reaches every sidecar over a dedicated pair network, by sidecar name (`sql`, `tools` above); container ID and IP also work
* every sidecar can reach the work container (pair networks are symmetric - keep its listeners minimal)
* sidecars reach each other only via `peers:` (or both setting `egress: true`)
* sidecars have no internet access unless `egress: true`
* workspaces are isolated from each other

`peers:` lists sidecars this sidecar may reach:

* each relation gets its own network; sidecars resolve each other by name on it
* `a: peers [b]` equals `b: peers [a]`
* names must refer to sidecars in the same workspace
* merged as a deduplicated union across config layers (other lists are replaced): overlays can add edges, not remove them

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

⚠️ Rooz creates one network per workspace, one per sidecar and one per peer relation. Docker's default address pools allow only ~31 networks host-wide. On an address-pool error, set `default-address-pools` with a smaller subnet size (e.g. `"size": 24`) in `daemon.json`. Podman (netavark) is not affected.

## Keys

* `image` - container image
* `env` - environment variables
* `command` - override entrypoint (`ENTRYPOINT`)
* `args` - override entrypoint arguments (`CMD`)
* `mounts` - container path to a `data` key or inline `content:`/`file:`, as in the work container's `mounts`. Volumes survive container restarts/deletes:

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

* `ports` - port bindings, same format as [work container ports](configuration.md#port-mappings)
* `vpn` (`bool`) - grants `NET_ADMIN`, `/dev/net/tun` and `src_valid_mark` for in-sidecar VPNs, with operator consent (see [VPN sidecars](configuration.md#vpn-sidecars))
* `peers` - sidecars this one may reach (see [Networking](#networking))
* `egress` (`bool`, default `false`) - outbound internet access: joins the workspace's `<workspace>-egress` network, shared with the work container and other egress sidecars. Install steps always have internet access.
* `install` - install steps, built into an image (see [Sidecars](configuration.md#sidecars))
* `uid` - uid to run as (default `0`)
* `shell` - shell for `rooz enter --container` (default `sh`)
* `init` (`bool`, default `true`) - run an init process as pid 1
* `privileged` (`bool`) - with operator consent (see [Privileged containers](configuration.md#privileged-containers))
* `work_dir` - working directory
