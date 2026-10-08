#!/bin/sh
# Installs herdr-inbox from its GitHub releases:
#
#   curl -fsSL https://raw.githubusercontent.com/lucasscariot/herdr-inbox/main/install.sh | sh
#
# It downloads the binary for this system, checks its SHA-256, and puts it in
# ~/.local/bin (HERDR_INBOX_INSTALL_DIR to change). It never touches herdr.
#
# HERDR_INBOX_VERSION picks a release tag (default: the latest).
# HERDR_INBOX_BASE_URL serves the assets from elsewhere (mirrors, tests).
set -eu

repo="${HERDR_INBOX_REPO:-lucasscariot/herdr-inbox}"
version="${HERDR_INBOX_VERSION:-latest}"
install_dir="${HERDR_INBOX_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { printf 'herdr-inbox install: %s\n' "$*" >&2; exit 1; }

os="${HERDR_INBOX_OS:-$(uname -s)}"
arch="${HERDR_INBOX_ARCH:-$(uname -m)}"
case "$os" in
  Linux | linux) os=linux ;;
  Darwin | darwin | macos) os=macos ;;
  *) fail "no prebuilt binary for $os. Build from source: cargo install --locked --git https://github.com/$repo" ;;
esac
case "$arch" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) fail "no prebuilt binary for $arch. Build from source: cargo install --locked --git https://github.com/$repo" ;;
esac

asset="herdr-inbox-$os-$arch.tar.gz"
if [ -n "${HERDR_INBOX_BASE_URL:-}" ]; then
  base="$HERDR_INBOX_BASE_URL"
elif [ "$version" = latest ]; then
  base="https://github.com/$repo/releases/latest/download"
else
  base="https://github.com/$repo/releases/download/$version"
fi

download() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$1" -O "$2"
  else
    fail "curl or wget is needed to download herdr-inbox"
  fi
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    fail "sha256sum or shasum is needed to check the download"
  fi
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

say "Downloading $asset…"
download "$base/$asset" "$work/$asset" || fail "could not download $base/$asset"
download "$base/$asset.sha256" "$work/$asset.sha256" || fail "could not download the checksum"
expected=$(cut -d ' ' -f 1 < "$work/$asset.sha256")
actual=$(sha256 "$work/$asset")
[ -n "$expected" ] && [ "$expected" = "$actual" ] || fail "checksum mismatch for $asset: nothing was installed"

tar -xzf "$work/$asset" -C "$work"
[ -f "$work/herdr-inbox" ] || fail "the archive has no herdr-inbox binary"

mkdir -p "$install_dir"
# Copy next to the target, then rename: a running herdr-inbox keeps working
# and a failed copy never leaves half a binary.
cp "$work/herdr-inbox" "$install_dir/.herdr-inbox.new"
chmod 755 "$install_dir/.herdr-inbox.new"
mv -f "$install_dir/.herdr-inbox.new" "$install_dir/herdr-inbox"

installed=$("$install_dir/herdr-inbox" --version 2>/dev/null || echo herdr-inbox)
say "Installed $installed to $install_dir/herdr-inbox"

case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) say "Note: $install_dir is not on your PATH. Add it to run herdr-inbox from anywhere." ;;
esac
if ! command -v herdr >/dev/null 2>&1; then
  say "Note: herdr-inbox runs on Herdr 0.9.2 or newer, which is not installed yet: https://herdr.dev"
fi
