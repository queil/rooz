# Sidecars

*It's similar to docker-compose but super simple and limited to bare minimum.*

* `rooz` has a limited support for sidecars (containers running along). It is only available via the config file

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
  `image` may be omitted in individual config layers (e.g. an overlay extending a sidecar defined in a base via `bases:`) but must be set for each sidecar once all layers are merged, otherwise creating the workspace fails.

## Networking

Workspace networking follows a hub-and-spoke model:

* the work container can reach every sidecar — it shares a dedicated pair network with each one
* a sidecar can always reach the work container (pair-network reachability is symmetric; keep listeners in the work container minimal)
* sidecars cannot reach each other unless explicitly connected via `peers:`
* workspaces are isolated from each other

`peers:` lists other sidecar names a sidecar may talk to. Each declared relation gets its own network, `a: peers [b]` and `b: peers [a]` are equivalent, and peer names must refer to sidecars defined in the same workspace. Sidecars resolve each other by name over their peer network:

  ```yaml
  sidecars:
    claude:
      peers: [proxy]   # claude's only egress path is the proxy
    proxy:
      egress: true     # reachable only by the work container and claude
    dkr:
      peers: [images]  # docker-in-docker pulling via the mirror
    images:
      egress: true     # registry pull-through cache
  ```

  Unlike other list fields (replaced by higher config layers), `peers` merge as a deduplicated union across layers: an overlay can add reachability edges but cannot remove inherited ones.

  ⚠️ rooz creates one network per sidecar plus one per peer relation. Docker's default address pools allow only ~31 networks host-wide; if network creation fails with an address-pool error, configure `default-address-pools` with a smaller subnet size (e.g. `"size": 24`) in `daemon.json`. Podman (netavark) is not affected.

  The work container addresses sidecars by their names (`sql` and `tools` in the first example above). The usual container ID and IP work too, but are not as convenient.

* the `enter` command lets you specify `--container` to enter (otherwise it enters the work container).

Supported keywords:
* `image` - set containers image
* `env` - set environment variables
* `command` - override container entrypoint (`ENTRYPOINT` in Dockerfile)
* `args` - override container entrypoint arguments (`CMD` in Dockerfile)
* `mounts` - mount automatically-named rw volumes at the specified paths (so they can survive container restarts/deletes). It supports:

```yaml
  mounts:
  # simple empty rw volumes
  - /my_test/data/
  # config files with a specified content
  - mount: /etc/tool/
    files:
      config.json: |-
        {"some": "config"}
      other.yaml: |-
        some: config
```

* `ports` - port bindings in the `"8080:8080"` format
* `vpn` (`bool`) - grants `NET_ADMIN`, `/dev/net/tun` and `src_valid_mark` for in-sidecar VPNs, with operator consent (see [VPN sidecars](configuration.md#vpn-sidecars))
* `peers` - names of other sidecars this sidecar may reach (see [Networking](#networking); merged as a union across config layers)
* `work_dir` - set working directory
* `mount_work` (`bool`) - if true then the work volume is mounted at `/work`

