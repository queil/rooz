#!/usr/bin/env bash
# Print one version's section of CHANGELOG.md - the release notes for that tag.
#
# Usage:
#   scripts/changelog-section.sh v0.159.0   # leading 'v' optional
#
# Exits non-zero when the version has no section, which is what makes the release
# workflow refuse a tag nobody wrote notes for.

set -euo pipefail

VERSION="${1:?usage: changelog-section.sh <version>}"
VERSION="${VERSION#v}"
CHANGELOG="$(dirname "$0")/../CHANGELOG.md"

section="$(awk -v version="$VERSION" '
  # section heads look like "## [0.159.0] - 2026-09-24" or "## [0.159.0]"
  /^## / {
    if (printing) { exit }
    heading = $0
    sub(/^## +/, "", heading)
    sub(/^\[/, "", heading)
    sub(/\].*$/, "", heading)
    sub(/ .*$/, "", heading)
    if (heading == version) { printing = 1; next }
  }
  printing { print }
' "$CHANGELOG")"

# strip leading and trailing blank lines
section="$(printf '%s\n' "$section" | sed -e '/./,$!d' -e ':a' -e '/^\n*$/{$d;N;ba' -e '}')"

if [ -z "$section" ]; then
  echo "No CHANGELOG.md section for version '$VERSION'" >&2
  exit 1
fi

printf '%s\n' "$section"
