#!/usr/bin/env bash
# Package a checked binary using the installer's archive and checksum names.
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo 'Usage: package-release.sh <binary> <linux-x86_64|linux-aarch64|macos-x86_64|macos-aarch64> [output-directory]' >&2
  exit 2
fi
binary=$1
asset=$2
output=${3:-.}
case "$asset" in
  linux-x86_64|linux-aarch64|macos-x86_64|macos-aarch64) ;;
  *) echo "Unsupported release asset: $asset" >&2; exit 2 ;;
esac
[[ -f "$binary" && -x "$binary" ]] || { echo "Not an executable file: $binary" >&2; exit 1; }
root=$(cd "$(dirname "$0")/.." && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$output"
cp "$binary" "$stage/herdr-inbox"
cp "$root/LICENSE" "$root/README.md" "$stage/"
name="herdr-inbox-$asset.tar.gz"
tar -czf "$output/$name" -C "$stage" herdr-inbox LICENSE README.md
(
  cd "$output"
  if command -v sha256sum >/dev/null; then
    sha256sum "$name" > "$name.sha256"
  else
    shasum -a 256 "$name" > "$name.sha256"
  fi
)
printf 'Packaged %s with SHA-256\n' "$output/$name"
