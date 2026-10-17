# Notes and known issues

## Other facts

* cloned repos are in `/work/{repo_name}`, `repo_name` being git's default clone directory name
* `rooz new`/`update` print one `[phase]` line per step (image, volumes, clone, container, install) to stderr. `-v`/`--verbose` adds debug logging; `RUST_LOG=rooz` also works
* `rooz system prune` removes ALL rooz containers and volumes. `rooz rm --all --force` removes only workspaces, keeping shared cache volumes and the SSH key volume

  ⚠️ `rooz system prune` deletes all rooz state. Back up anything important first.

## Upgrading

* cache volume names now include an operator scope (`rooz_cache_<scope>_<path>`), so caches from earlier versions are orphaned. They repopulate automatically; `rooz system prune` removes the leftovers.

## Known issues

* the session doesn't auto-resize with the terminal. Workaround: exit, resize the window, enter again.
