# Install

```sh
curl -fsSL https://rooz.dev/install.sh | sh
```

The script installs the latest release to `~/.local/bin/rooz` (`ROOZ_INSTALL_DIR` overrides it) on
Linux and macOS, x86_64 or arm64. It pins one version and carries that release's checksums, so the
binary is verified against data served from rooz.dev rather than from the GitHub release it
downloads from. If the GitHub CLI is installed and logged in, it also verifies the binary's signed
build provenance.

## Manually

Download the binary for your platform from the
[latest release](https://github.com/queil/rooz/releases/latest):

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

Every release carries `SHA256SUMS`, and the binaries have signed build provenance
(`gh attestation verify ./rooz --repo queil/rooz`).

Next: [Getting started](getting-started.md).
