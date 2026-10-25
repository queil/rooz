# joey

rooz's helper: a small static binary for the containers rooz starts.

## `joey egress-proxy --config <file>`

The only way out for the containers that peer with it. It speaks HTTP `CONNECT` and nothing else.

* A `CONNECT` is decided on its host and port before anything is resolved or dialled: a host or
  port outside the lists gets `403`.
* An `allow` host is reached as it is: the TLS session is the client's and the server's. The
  ClientHello must ask for the host the `CONNECT` named, so an allowed tunnel cannot carry another
  server name.
* An `inject` host is intercepted: the proxy presents a certificate from `ca` (which the clients
  trust) and applies the first matching rule's header changes before passing the request on over
  verified TLS. The request goes in origin-form with its `Host` header set to that host, whatever
  target or `Host` the client sent. Its path must already be canonical - no `.` or `..` segments,
  repeated `/`, encoded dots or separators, or backslashes - or it is answered `400` and goes
  nowhere, so a rule matches the path the upstream will route.
* Plain HTTP requests are refused.
* A config it cannot read, including a `ca` it cannot load, stops it at startup: it never runs
  open. Every decision is a line on stdout.

```json
{
  "listen": "0.0.0.0:9124",
  "allow": ["github.com", "*.githubusercontent.com", "crates.io"],
  "ports": [443],
  "ca": {"cert": "/etc/joey/ca.pem", "key": "/etc/joey/ca.key"},
  "upstream_roots": [],
  "inject": [
    {
      "host": "api.anthropic.com",
      "rules": [
        {
          "methods": ["POST"],
          "paths": ["/v1/*"],
          "when_header": "x-cc-req",
          "set": {"authorization": "Bearer <token>"},
          "remove": ["x-cc-req"]
        }
      ]
    }
  ]
}
```

* `allow`: host names, or `*.` followed by one for the hosts under it (not the name itself).
* `ports`: `[443]` unless set.
* `inject`: exact host names, none of them also in `allow`. A rule matches on `methods` (any
  when empty), `paths` (any when empty; `/v1/*` is a prefix, which ends at a `/`, anything else
  the exact path) and `when_header` (a header the request must carry - the client sets it, so it
  selects requests but restricts no one). A request no rule matches goes on unchanged. `paths`
  narrows where a credential goes; it is no substitute for the upstream's own authorization.
* `upstream_roots`: PEM files trusted for upstream servers on top of the built-in web roots.
