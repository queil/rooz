# Running with Podman

1. Make sure podman remote socket is enabled:

`podman info` should contain the following YAML:

```yaml
remoteSocket:
    exists: true
    path: /run/user/1000/podman/podman.sock
```

If `exists: true` is missing, try this command: `systemctl --user enable --now podman.socket`

2. Make sure you have the podman socket exposed as the `DOCKER_HOST` env var like:

```
export DOCKER_HOST=unix:///run/user/1000/podman/podman.sock
```

3. Use fully-qualified image names or define unqualified-search registries `/etc/containers/registries.conf`

4. When running more complex podman in podman scenarios (like networking) you may need to run rooz with `--privileged` switch
   [more info](https://www.redhat.com/sysadmin/privileged-flag-container-engines).

