#!/usr/bin/env bash
# Builds joey's static image from this checkout and loads it into the test daemon at
# $ROOZ_TEST_DOCKER_HOST (what test-daemon.sh exports), so the egress tests run the proxy from the
# commit under test - the released image rooz names does not exist before its release. The image
# goes in through the daemon's own API, which docker and podman both serve.
#
# Usage (prints the variable to export):
#   ROOZ_TEST_DOCKER_HOST=tcp://... bash scripts/joey-test-image.sh

set -euo pipefail

: "${ROOZ_TEST_DOCKER_HOST:?the test daemon, as test-daemon.sh exports it}"
TAG="localhost/joey:test"
TARGET="$(uname -m)-unknown-linux-gnu"
CTX="$(mktemp -d)"

RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --locked \
  --manifest-path joey/Cargo.toml --target "$TARGET" >&2
cp "joey/target/$TARGET/release/joey" "$CTX/joey"
docker build -q -f joey/Dockerfile.scratch -t "$TAG" "$CTX" >&2

docker save -o "$CTX/joey.tar" "$TAG"
DOCKER_HOST="$ROOZ_TEST_DOCKER_HOST" docker load -i "$CTX/joey.tar" >&2
rm -rf "$CTX"

echo "export ROOZ_EGRESS_IMAGE=$TAG"
