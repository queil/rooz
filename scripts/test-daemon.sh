#!/usr/bin/env bash
# Boot or teardown an isolated test daemon for integration tests.
#
# Usage:
#   scripts/test-daemon.sh up   [docker|podman]   # start daemon, print env exports
#   scripts/test-daemon.sh down [docker|podman]   # stop and remove daemon container
#
# After "up", eval the printed exports to configure the test environment:
#   eval "$(scripts/test-daemon.sh up docker)"
#   cargo test --test smoke --test lifecycle --test volumes --test sidecars -- --test-threads=1
#
# Requires:
#   - docker available on PATH
#   - Sufficient privileges to run --privileged containers

set -euo pipefail

ENGINE="${2:-docker}"
CONTAINER_NAME="rooz-test-${ENGINE}"

case "${1:-}" in
up)
  case "$ENGINE" in
  docker)
    # docker:dind-rootless requires nested user-namespace support which is not
    # available when the outer daemon is itself rootless.  Use docker:dind
    # (rootful) instead — the integration tests exercise the Docker API, not
    # rootless-specific kernel behaviour.
    #
    # Publish port to the docker host so tests can reach the daemon even when
    # running on a different machine (e.g. dev container + remote docker host).
    docker run -d --name "$CONTAINER_NAME" \
      --privileged \
      -e DOCKER_TLS_CERTDIR="" \
      -p 2375 \
      docker:dind \
      dockerd --host=tcp://0.0.0.0:2375 --tls=false

    ELAPSED=0
    until docker exec "$CONTAINER_NAME" docker -H tcp://localhost:2375 info >/dev/null 2>&1; do
      sleep 1
      ELAPSED=$((ELAPSED + 1))
      if [ "$ELAPSED" -ge 60 ]; then
        echo "Timed out waiting for dockerd. Logs:" >&2
        docker logs "$CONTAINER_NAME" >&2
        exit 1
      fi
    done

    PORT=$(docker port "$CONTAINER_NAME" 2375 | cut -d: -f2)
    if echo "${DOCKER_HOST:-}" | grep -q '^tcp://'; then
      HOST=$(echo "$DOCKER_HOST" | sed 's|tcp://||' | cut -d: -f1)
    else
      HOST="localhost"
    fi
    echo "export ROOZ_TEST_DOCKER_HOST=tcp://${HOST}:${PORT}"
    echo "export ROOZ_TEST_ENGINE=docker"
    ;;

  podman)
    IMAGE="quay.io/podman/stable:v6"
    docker run -d --name "$CONTAINER_NAME" \
      --privileged \
      --security-opt seccomp=unconfined \
      --security-opt apparmor=unconfined \
      --device /dev/fuse \
      -p 2375 \
      "$IMAGE" \
      sh -c "podman system service --time=0 tcp://0.0.0.0:2375 2>&1"

    ELAPSED=0
    until docker exec "$CONTAINER_NAME" podman info >/dev/null 2>&1; do
      sleep 1
      ELAPSED=$((ELAPSED + 1))
      if [ "$ELAPSED" -ge 120 ]; then
        echo "Timed out waiting for podman service. Logs:" >&2
        docker logs "$CONTAINER_NAME" >&2
        exit 1
      fi
    done

    PORT=$(docker port "$CONTAINER_NAME" 2375 | cut -d: -f2)
    if echo "${DOCKER_HOST:-}" | grep -q '^tcp://'; then
      HOST=$(echo "$DOCKER_HOST" | sed 's|tcp://||' | cut -d: -f1)
    else
      HOST="localhost"
    fi
    echo "export ROOZ_TEST_DOCKER_HOST=tcp://${HOST}:${PORT}"
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
