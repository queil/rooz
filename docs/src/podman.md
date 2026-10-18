# Podman

1. Enable Podman's API socket. `podman info` should show:

   ```yaml
   remoteSocket:
       exists: true
       path: /run/user/1000/podman/podman.sock
   ```

   If `exists: true` is missing, run `systemctl --user enable --now podman.socket`.

2. Point `DOCKER_HOST` at the socket:

   ```sh
   export DOCKER_HOST=unix:///run/user/1000/podman/podman.sock
   ```

3. Use fully qualified image names, or set `unqualified-search-registries` in
   `/etc/containers/registries.conf`.

Nested Podman setups (e.g. with their own networking) may need `--privileged`
([why](https://www.redhat.com/sysadmin/privileged-flag-container-engines)).
