# Install

```sh
curl -fsSL https://rooz.dev/install.sh | sh
```

Installs the latest release to `~/.local/bin/rooz` (override with `ROOZ_INSTALL_DIR`). Supports Linux
and macOS, x86_64 and arm64. The script pins one version and embeds its checksums, so the binary is
verified against data from rooz.dev, not from the GitHub release. If the GitHub CLI is installed and
logged in, it also verifies the signed build provenance.

## Manually

Download the binary from the [latest release](https://github.com/queil/rooz/releases/latest):

| Platform              | Asset                              |
|-----------------------|------------------------------------|
| Linux x86_64          | `rooz-x86_64-unknown-linux-gnu`    |
| Linux arm64           | `rooz-aarch64-unknown-linux-gnu`   |
| macOS (Apple silicon) | `rooz-aarch64-apple-darwin`        |
| macOS (Intel)         | `rooz-x86_64-apple-darwin`         |

```sh
curl -fsSLO https://github.com/queil/rooz/releases/latest/download/rooz-x86_64-unknown-linux-gnu
curl -fsSLO https://github.com/queil/rooz/releases/latest/download/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
install -m 755 rooz-x86_64-unknown-linux-gnu ~/.local/bin/rooz
```

Every release has `SHA256SUMS` and signed build provenance; verify the latter with `gh attestation verify ./rooz-x86_64-unknown-linux-gnu --repo queil/rooz`.

Next: [Getting started](getting-started.md).
