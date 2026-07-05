#!/usr/bin/env bash
# Boot or teardown a rootless test daemon for integration tests.
#
# Usage:
#   scripts/test-daemon.sh up   [docker|podman]   # start daemon, print env exports
#   scripts/test-daemon.sh down [docker|podman]   # stop and remove daemon container
#
# After "up", eval the printed exports to configure the test environment:
#   eval "$(scripts/test-daemon.sh up docker)"
#   cargo test --test smoke
#
# Requires:
#   - docker available on PATH
#   - Sufficient privileges to run --privileged containers (needed for dind-rootless /
#     podman svc inside a container)

set -euo pipefail

ENGINE="${2:-docker}"
CONTAINER_NAME="rooz-test-${ENGINE}"

case "${1:-}" in
up)
  case "$ENGINE" in
  docker)
    IMAGE="docker:dind-rootless"
    docker run -d --name "$CONTAINER_NAME" \
      --privileged \
      -e DOCKER_TLS_CERTDIR="" \
      "$IMAGE" \
      dockerd-entrypoint.sh --host=tcp://0.0.0.0:2375 --tls=false

    # Wait for the daemon to be ready
    until docker exec "$CONTAINER_NAME" docker info >/dev/null 2>&1; do
      sleep 1
    done

    IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$CONTAINER_NAME")
    echo "export ROOZ_TEST_DOCKER_HOST=tcp://${IP}:2375"
    echo "export ROOZ_TEST_ENGINE=docker"
    ;;

  podman)
    IMAGE="quay.io/podman/stable:v6"
    docker run -d --name "$CONTAINER_NAME" \
      --privileged \
      --security-opt seccomp=unconfined \
      --security-opt apparmor=unconfined \
      --device /dev/fuse \
      "$IMAGE" \
      sh -c "podman system service --time=0 tcp://0.0.0.0:2375 2>&1"

    # Wait for the socket to be ready
    until docker exec "$CONTAINER_NAME" podman info >/dev/null 2>&1; do
      sleep 1
    done

    IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$CONTAINER_NAME")
    echo "export ROOZ_TEST_DOCKER_HOST=tcp://${IP}:2375"
    echo "export ROOZ_TEST_ENGINE=podman"
    ;;

  *)
    echo "Unknown engine: $ENGINE. Use 'docker' or 'podman'." >&2
    exit 1
    ;;
  esac
  ;;

down)
  docker rm -f "$CONTAINER_NAME" 2>/dev/null || true
  ;;

*)
  echo "Usage: $0 {up|down} [docker|podman]" >&2
  exit 1
  ;;
esac
