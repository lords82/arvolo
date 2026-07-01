#!/bin/sh
# Arvolo installer — downloads the prebuilt `arvolo` (and `arvolo-relay`) binaries
# for your platform from the latest GitHub Release and installs them.
#
#   curl -fsSL https://raw.githubusercontent.com/lords82/arvolo/main/install.sh | sh
#
# Overrides (environment):
#   ARVOLO_VERSION       tag to install, e.g. v0.1.0 (default: latest published)
#   ARVOLO_INSTALL_DIR   install directory (default: /usr/local/bin, else ~/.local/bin)
#
# POSIX sh, no bashisms. Prebuilt binaries currently cover Linux x86_64 and
# macOS arm64; other platforms fall back to `cargo install`.
set -eu

REPO="lords82/arvolo"
BINARIES="arvolo arvolo-relay"

info() { printf '%s\n' "$*" >&2; }
err() { printf 'error: %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || err "missing required tool: $1"; }

cargo_fallback() {
  info ""
  info "No prebuilt binary for this platform ($1). Install from source instead:"
  info "    cargo install --git https://github.com/$REPO arvolo-cli"
  info "    cargo install --git https://github.com/$REPO arvolo-relay"
  exit 1
}

# --- detect platform -------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)
    case "$arch" in
      x86_64 | amd64) label="linux-x86_64" ;;
      *) cargo_fallback "$os/$arch" ;;
    esac ;;
  Darwin)
    case "$arch" in
      arm64 | aarch64) label="macos-arm64" ;;
      *) cargo_fallback "$os/$arch (macOS Intel has no prebuilt binary)" ;;
    esac ;;
  *) cargo_fallback "$os/$arch" ;;
esac

# --- pick a download tool --------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  dl() { curl -fsSL "$1" -o "$2"; }
  fetch() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
  dl() { wget -qO "$2" "$1"; }
  fetch() { wget -qO- "$1"; }
else
  err "need curl or wget"
fi
need tar

# --- resolve version -------------------------------------------------------
tag="${ARVOLO_VERSION:-}"
if [ -z "$tag" ]; then
  info "Resolving the latest release…"
  tag="$(fetch "https://api.github.com/repos/$REPO/releases/latest" \
    | grep '"tag_name"' | head -n1 | cut -d'"' -f4)"
  [ -n "$tag" ] || err "could not determine the latest release (is one published, not just draft?). Set ARVOLO_VERSION=vX.Y.Z"
fi

asset="arvolo-${tag}-${label}.tar.gz"
base="https://github.com/$REPO/releases/download/$tag"
info "Installing arvolo $tag ($label)…"

# --- download + verify + extract ------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM
dl "$base/$asset" "$tmp/$asset" || err "download failed: $base/$asset"

# Best-effort SHA256 verification against the release's SHA256SUMS.
sumtool=""
if command -v sha256sum >/dev/null 2>&1; then sumtool="sha256sum"
elif command -v shasum >/dev/null 2>&1; then sumtool="shasum -a 256"; fi
if [ -n "$sumtool" ] && dl "$base/SHA256SUMS" "$tmp/SHA256SUMS" 2>/dev/null; then
  want="$(grep " $asset\$" "$tmp/SHA256SUMS" | awk '{print $1}' | head -n1)"
  if [ -n "$want" ]; then
    got="$($sumtool "$tmp/$asset" | awk '{print $1}')"
    [ "$want" = "$got" ] || err "checksum mismatch for $asset (expected $want, got $got)"
    info "Checksum OK."
  fi
else
  info "Skipping checksum verification (no SHA256SUMS or sha256 tool)."
fi

tar -xzf "$tmp/$asset" -C "$tmp"

# --- choose install dir ----------------------------------------------------
dir="${ARVOLO_INSTALL_DIR:-/usr/local/bin}"
sudo=""
if [ ! -d "$dir" ] || [ ! -w "$dir" ]; then
  if [ "${ARVOLO_INSTALL_DIR:-}" = "" ] && [ -w "/usr/local" ] 2>/dev/null; then
    :
  elif [ "${ARVOLO_INSTALL_DIR:-}" = "" ] && command -v sudo >/dev/null 2>&1 && [ -d "$dir" ]; then
    sudo="sudo"
  else
    dir="$HOME/.local/bin"
    mkdir -p "$dir"
  fi
fi

for b in $BINARIES; do
  [ -f "$tmp/$b" ] || continue
  $sudo install -m 0755 "$tmp/$b" "$dir/$b"
  info "Installed $dir/$b"
done

# --- PATH hint -------------------------------------------------------------
case ":$PATH:" in
  *":$dir:"*) ;;
  *) info ""
     info "Note: $dir is not on your PATH. Add it, e.g.:"
     info "    export PATH=\"$dir:\$PATH\"" ;;
esac

info ""
info "Done. Try: arvolo --help"
