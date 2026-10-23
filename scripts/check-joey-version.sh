#!/usr/bin/env bash
# joey is released with rooz and carries its version: both manifests must say the same.
set -euo pipefail

root="$(dirname "$0")/.."
version() { sed -n 's/^version = "\(.*\)"/\1/p' "$1" | head -n1; }

rooz="$(version "$root/Cargo.toml")"
joey="$(version "$root/joey/Cargo.toml")"

[ "$rooz" = "$joey" ] || {
  echo "joey/Cargo.toml says $joey, Cargo.toml says $rooz: bump both" >&2
  exit 1
}
echo "joey and rooz are both $rooz"
