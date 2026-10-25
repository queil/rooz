#!/usr/bin/env bash
# Builds joey's static image from this checkout and loads it into the test daemon started by
# test-daemon.sh, so the egress tests run the proxy from the commit under test - the released
# image rooz names does not exist before its release.
#
# Usage (prints the variable to export):
#   bash scripts/joey-test-image.sh docker|podman

set -euo pipefail

ENGINE="${1:-docker}"
TAG="localhost/joey:test"
TARGET="$(uname -m)-unknown-linux-gnu"
CTX="$(mktemp -d)"

RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --locked \
  --manifest-path joey/Cargo.toml --target "$TARGET" >&2
cp "joey/target/$TARGET/release/joey" "$CTX/joey"
docker build -q -f joey/Dockerfile.scratch -t "$TAG" "$CTX" >&2

case "$ENGINE" in
docker) docker save "$TAG" | docker exec -i rooz-test-docker docker -H tcp://localhost:2375 load >&2 ;;
podman) docker save "$TAG" | docker exec -i rooz-test-podman podman load >&2 ;;
*)
  echo "unknown engine: $ENGINE" >&2
  exit 1
  ;;
esac
rm -rf "$CTX"

echo "export ROOZ_EGRESS_IMAGE=$TAG"
