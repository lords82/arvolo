#!/usr/bin/env bash
# The six places the version is written must agree.
#
# Run by CI, because the failure this catches has no other symptom. A stale
# `gui/src-tauri/tauri.conf.json` does not break the build, does not break the
# app, and does not break the release — it just means every installed copy
# compares itself against the wrong number and is never offered the update.
#
# Also run at the end of `bump-version.sh`, so the script that writes them all
# is checked by the same rule that guards them.

set -euo pipefail

cd "$(dirname "$0")/.."

workspace=$(perl -ne 'print "$1\n" and exit if /^version = "([^"]+)"$/m' Cargo.toml)
core_dep=$(perl -ne 'print "$1\n" and exit if /arvolo-core = \{ path = "core", version = "([^"]+)"/' Cargo.toml)
engine_dep=$(perl -ne 'print "$1\n" and exit if /arvolo-engine = \{ path = "engine", version = "([^"]+)"/' Cargo.toml)
ipc_dep=$(perl -ne 'print "$1\n" and exit if /arvolo-ipc = \{ path = "arvolo-ipc", version = "([^"]+)"/' Cargo.toml)
pkg=$(perl -ne 'print "$1\n" and exit if /"version": "([^"]+)"/' gui/package.json)
tauri=$(perl -ne 'print "$1\n" and exit if /"version": "([^"]+)"/' gui/src-tauri/tauri.conf.json)

printf '%-34s %s\n' \
  "Cargo.toml [workspace.package]" "$workspace" \
  "Cargo.toml arvolo-core dep" "$core_dep" \
  "Cargo.toml arvolo-engine dep" "$engine_dep" \
  "Cargo.toml arvolo-ipc dep" "$ipc_dep" \
  "gui/package.json" "$pkg" \
  "gui/src-tauri/tauri.conf.json" "$tauri"

status=0
for v in "$core_dep" "$engine_dep" "$ipc_dep" "$pkg" "$tauri"; do
  [ "$v" = "$workspace" ] || status=1
done

if [ "$status" -ne 0 ]; then
  echo
  echo "these do not all say the same thing — run scripts/bump-version.sh <version>" >&2
  echo "(the tauri.conf.json one is the number the in-app updater compares)" >&2
  exit 1
fi

echo
echo "all six agree on $workspace"
