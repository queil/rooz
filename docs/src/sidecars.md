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
              set: {authorization: "Bearer {{ anthropic_token }}"}
              remove: [x-cc-req]
```

* rooz adds a sidecar `<name>-egress` running joey's proxy (`joey egress-proxy`). It is the only one of
  the two with a way out; the sidecar reaches it, and nothing else outside the workspace, through
  their peer network. `HTTP(S)_PROXY` and `NO_PROXY` (itself and its peers) are set in the sidecar
  unless its `env` sets them. The proxy URL carries a credential generated for that proxy, and the
  proxy refuses (`407`) any client without it - the work container reaches the proxy too, and must not
  spend what it holds. A tool that ignores credentials in a proxy URL cannot get out.
* `allow`: hosts reached through as they are, TLS end to end. `*.example.com` is the hosts under
  `example.com`, not `example.com` itself. The proxy refuses any other host, and any port but `ports`
  (`[443]` unless set), before it is even resolved, and checks that a connection's TLS server name is
  the host it was opened for.
* `inject`: hosts the proxy intercepts to apply header rules - exact names, never also in `allow`,
  though an `allow` wildcard may cover one (`*.github.com` and `nuget.pkg.github.com`). The first rule
  a request matches by `methods`, `paths` (`/v1/*` is a prefix ending at a `/`) and `when_header` sets
  and removes headers.
  A request to an inject host goes in origin-form and only on a canonical path (no `..`, `.`, `//`,
  `;` path parameters or encoded dots and separators), so a rule matches what the upstream routes. The
  values, typically from `secrets`, exist only in the proxy. The proxy presents certificates from a
  CA generated for the workspace: its certificate is mounted in the sidecar at
  `/usr/local/share/ca-certificates/rooz-egress.crt` and named by `NODE_EXTRA_CA_CERTS`; for tools
  that read the system store, add a `root: true` install step running `update-ca-certificates`. The
  CA is written once and kept across updates, and its key is never stored in the workspace's config.
* Plain HTTP is refused.
* `learn: true` lets through what `allow` and `ports` would refuse, and logs it as `learn`: open egress
  while you find out what the sidecar needs. An `inject` host is still held to `ports`, so a
  credential never goes anywhere new. Turn it off once the allowlist is in place.
* Across config layers, two policies combine: `allow` and `ports` are unions (a layer without `ports`
  counts as `[443]`), and `inject` goes by host, a later layer's entry for a host replacing the earlier
  one. A host a later layer injects leaves the lower layers' `allow`, and one it allows leaves their
  `inject`. `true` or `false` replaces a policy below it, and a policy replaces them. So a base can
  carry the allowlist and your own config the `inject` rules with their secrets.

### Egress log

Every decision the proxy makes is a line of JSON in its container's log. `rooz egress log` reads them:

```sh
rooz egress log <workspace>               # per proxy: decisions counted by host, refusals first
rooz egress log <workspace> --denied      # only what was refused, or let through only by learning
rooz egress log <workspace> --suggest     # the refused hosts as a policy to paste into the config
rooz egress log <workspace> -c claude -f  # one sidecar's decisions as they happen
```

Each row names the client's User-Agent (`ua:`). The client names itself, so it is a hint, not proof.
Some clients (.NET) send the proxy credential only once challenged. A refusal for a *missing* credential
is shown as `auth`, a challenge answered, when the same client address - assigned by the network, not
claimed by the client - gets through to the same host after it in the log. Any other credential
refusal, a wrong credential included, stays `deny`. `-f` shows every refusal as it happens.
`--tail <n>` reads only the last `n` log lines.
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
