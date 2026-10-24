# Notes

* Cloned repositories live in `/work/<repo_name>`, where `repo_name` is the directory git would clone
  into by default.
* Clones are full, not partial, so `git` works offline afterwards. Each repository is kept as a bare
  mirror in a cache volume (`rooz cache ls`), refreshed on every clone, and the clone borrows its objects, so
  a second workspace of the same repository clones in seconds. Only rooz's git container mounts the volume;
  never list its path in `caches:`. A mirror that cannot be updated costs only the speed-up.
* `rooz new` and `rooz update` print one `[phase]` line per step (image, volumes, clone, container,
  install) to stderr. `-v`/`--verbose` adds debug logging; so does `RUST_LOG=rooz`.
* `rooz rm --all --force` removes every workspace but keeps shared cache volumes and the SSH key volume.
* `rooz system prune` removes **all** rooz containers and volumes.

> [!WARNING]
> `rooz system prune` deletes all rooz state, caches and the SSH key included. Back up anything you
> need first.
