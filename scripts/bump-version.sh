#!/usr/bin/env bash
# Set the release version everywhere it is written down.
#
# It lives in six places across three files, and until the in-app updater
# existed, forgetting one of them was cosmetic. It is not any more: the version
# in `gui/src-tauri/tauri.conf.json` is the number an installed copy compares
# against `latest.json`, so a release that forgets it is a release nobody is
# ever offered — and nothing fails, anywhere, to say so.
#
#   scripts/bump-version.sh 0.13.0
#
# Cargo parses these as semver, so a prerelease is written the semver way
# (`0.13.0-rc1`). The `~` form the Debian and RPM packagers need is derived from
# the tag inside the release workflow and never appears in a manifest.

set -euo pipefail

cd "$(dirname "$0")/.."

version="${1:-}"
if [ -z "$version" ]; then
  echo "usage: $0 <version>   e.g. $0 0.13.0 or $0 0.13.0-rc1" >&2
  exit 2
fi
# Deliberately strict, and deliberately refusing a leading "v": the tag carries
# one and none of these files does, which is exactly the sort of near-miss that
# would produce a manifest nothing matches.
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
  echo "not a version this can write: $version (expected 0.13.0, or 0.13.0-rc1)" >&2
  exit 2
fi

say() { printf '  %s\n' "$1"; }

echo "setting version to $version"

# The workspace version, plus the three path dependencies that repeat it — they
# carry a version as well as a path so the binaries can be published to
# crates.io against a released core.
perl -0pi -e "s/^version = \"[^\"]+\"$/version = \"$version\"/m" Cargo.toml
perl -0pi -e "s/(arvolo-core = \{ path = \"core\", version = )\"[^\"]+\"/\${1}\"$version\"/" Cargo.toml
perl -0pi -e "s/(arvolo-ipc = \{ path = \"arvolo-ipc\", version = )\"[^\"]+\"/\${1}\"$version\"/" Cargo.toml
perl -0pi -e "s/(arvolo-engine = \{ path = \"engine\", version = )\"[^\"]+\"/\${1}\"$version\"/" Cargo.toml
say "Cargo.toml"

# Only the first "version" key — package.json has plenty of others further down.
perl -0pi -e "s/(\"version\": )\"[^\"]+\"/\${1}\"$version\"/" gui/package.json
say "gui/package.json"

# The one the updater actually compares.
perl -0pi -e "s/(\"version\": )\"[^\"]+\"/\${1}\"$version\"/" gui/src-tauri/tauri.conf.json
say "gui/src-tauri/tauri.conf.json"

# Lockfiles, so the tree is consistent and `--locked` builds still work.
cargo update -w --quiet
say "Cargo.lock"
(cd gui && npm install --package-lock-only --silent)
say "gui/package-lock.json"

echo
echo "now check them:"
scripts/check-versions.sh
