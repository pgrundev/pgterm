#!/bin/sh
# pgterm installer — https://pgterm.dev/install.sh
#
#   curl -fsSL https://pgterm.dev/install.sh | sh
#
# Environment:
#   PGTERM_VERSION      version to install (default: latest release)
#   PGTERM_INSTALL_DIR  where the binary goes (default: /usr/local/bin)
set -eu

REPO="pgrundev/pgterm"
INSTALL_DIR="${PGTERM_INSTALL_DIR:-/usr/local/bin}"
VERSION="${PGTERM_VERSION:-latest}"

say() { printf 'pgterm-install: %s\n' "$*"; }
die() { printf 'pgterm-install: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

os=$(uname -s | tr '[:upper:]' '[:lower:]')
case "$os" in
  linux|darwin) ;;
  *) die "unsupported OS: $os (linux and macOS only — build from source: cargo install --git https://github.com/$REPO)" ;;
esac

case "$(uname -m)" in
  x86_64|amd64) arch=amd64 ;;
  arm64|aarch64) arch=arm64 ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac

if have curl; then
  fetch() { curl -fsSL "$1" -o "$2"; }
  fetch_stdout() { curl -fsSL "$1"; }
elif have wget; then
  fetch() { wget -qO "$2" "$1"; }
  fetch_stdout() { wget -qO- "$1"; }
else
  die "need curl or wget"
fi

if have sha256sum; then
  checksum() { sha256sum -c -; }
elif have shasum; then
  checksum() { shasum -a 256 -c -; }
else
  die "need sha256sum or shasum to verify the download"
fi

if [ "$VERSION" = "latest" ]; then
  VERSION=$(fetch_stdout "https://api.github.com/repos/$REPO/releases/latest" |
    sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
  [ -n "$VERSION" ] || die "could not determine the latest release (no releases yet?)"
fi
ver="${VERSION#v}"

base="https://github.com/$REPO/releases/download/v$ver"
name="pgterm_${ver}_${os}_${arch}"
tarball="$name.tar.gz"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

say "downloading $tarball"
fetch "$base/$tarball" "$tmp/$tarball"
fetch "$base/checksums.txt" "$tmp/checksums.txt"

say "verifying checksum"
(cd "$tmp" && grep " $tarball\$" checksums.txt | checksum >/dev/null) ||
  die "checksum verification FAILED — not installing"

tar -xzf "$tmp/$tarball" -C "$tmp"
bin="$tmp/$name/pgterm"
[ -f "$bin" ] || die "binary not found in archive"
chmod +x "$bin"

if [ -w "$INSTALL_DIR" ]; then
  mv "$bin" "$INSTALL_DIR/pgterm"
else
  say "installing to $INSTALL_DIR (needs sudo)"
  sudo mv "$bin" "$INSTALL_DIR/pgterm"
fi

say "installed: $("$INSTALL_DIR/pgterm" --version)"

if ! have pgbot; then
  say ""
  say "pgterm drives pgbot, which is not on your PATH yet. Install it with:"
  say ""
  say "  curl -fsSL https://pgbot.dev/install | sh"
fi

say ""
say "get started:"
say ""
say "  export DATABASE_URL='postgresql://...'"
say "  pgterm add production"
say "  pgterm"
