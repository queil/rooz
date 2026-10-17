# Running with Podman

1. Enable the podman remote socket. `podman info` should contain:

```yaml
remoteSocket:
    exists: true
    path: /run/user/1000/podman/podman.sock
```

If `exists: true` is missing, run `systemctl --user enable --now podman.socket`.

2. Expose the socket as `DOCKER_HOST`:

```
export DOCKER_HOST=unix:///run/user/1000/podman/podman.sock
```

3. Use fully-qualified image names, or define unqualified-search registries in `/etc/containers/registries.conf`.

4. Complex podman-in-podman scenarios (e.g. networking) may need `--privileged`
   ([more info](https://www.redhat.com/sysadmin/privileged-flag-container-engines)).
