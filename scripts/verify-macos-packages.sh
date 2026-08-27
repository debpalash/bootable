#!/usr/bin/env bash
set -euo pipefail

version="${1:?usage: verify-macos-packages.sh VERSION [OUTPUT_DIR]}"
output="${2:-dist/macos}"
root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
output="$root/$output"
dmg="$output/bootable-${version}-aarch64.dmg"
archive="$output/bootable-${version}-aarch64-apple-darwin.tar.gz"

(cd "$output" && shasum -a 256 -c ./*.sha256)
hdiutil verify "$dmg"
mount="$(mktemp -d "${TMPDIR:-/tmp}/bootable-dmg-verify.XXXXXX")"
extract="$(mktemp -d "${TMPDIR:-/tmp}/bootable-macos-verify.XXXXXX")"
cleanup() {
  hdiutil detach "$mount" -quiet >/dev/null 2>&1 || true
  rm -rf "$mount" "$extract"
}
trap cleanup EXIT
hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$mount" -quiet
test -d "$mount/Bootable.app"
test -x "$mount/Bootable.app/Contents/MacOS/bootable-desktop"
test -x "$mount/Bootable.app/Contents/MacOS/bootable"
test -x "$mount/Bootable.app/Contents/MacOS/bootable-helper"
test -x "$mount/Install Bootable Helper.command"
test "$(defaults read "$mount/Bootable.app/Contents/Info" CFBundleShortVersionString)" = "$version"
codesign --verify --deep --strict "$mount/Bootable.app"
test "$("$mount/Bootable.app/Contents/MacOS/bootable" --version)" = "bootable $version"
hdiutil detach "$mount" -quiet

tar -xzf "$archive" -C "$extract"
for executable in bootable bootable-desktop bootable-helper install.sh; do
  test -x "$extract/$executable"
done
test "$("$extract/bootable" --version)" = "bootable $version"
