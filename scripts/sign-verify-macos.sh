#!/usr/bin/env bash
# Assert that the macOS packages are Developer ID signed, hardened, notarized,
# and stapled. Run only when sign-macos.sh ran.
set -euo pipefail

version="${1:?usage: sign-verify-macos.sh VERSION [TARGET] [OUTPUT_DIR]}"
target="${2:-aarch64-apple-darwin}"
output="${3:-dist/macos}"
root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
output="$root/$output"
dmg="$output/bootable-${version}-aarch64.dmg"
archive="$output/bootable-${version}-${target}.tar.gz"

mount="$(mktemp -d "${TMPDIR:-/tmp}/bootable-dmg-sign-verify.XXXXXX")"
extract="$(mktemp -d "${TMPDIR:-/tmp}/bootable-archive-sign-verify.XXXXXX")"
cleanup() {
  hdiutil detach "$mount" -quiet >/dev/null 2>&1 || true
  rm -rf "$mount" "$extract"
}
trap cleanup EXIT

# Requires a Developer ID authority and a secure timestamp, plus the hardened
# runtime for executable code (disk images do not carry it).
assert_developer_id() {
  local path="$1" kind="${2:-code}" details
  codesign --verify --strict --verbose=2 "$path"
  details="$(codesign -dvv "$path" 2>&1)"
  grep -q '^Authority=Developer ID Application:' <<<"$details" ||
    { echo "$path is not signed by a Developer ID Application identity." >&2; exit 1; }
  grep -q '^Timestamp=' <<<"$details" ||
    { echo "$path has no secure timestamp." >&2; exit 1; }
  if [[ "$kind" == code ]]; then
    grep -Eq 'flags=0x[0-9a-f]+\(.*runtime' <<<"$details" ||
      { echo "$path does not enable the hardened runtime." >&2; exit 1; }
  fi
}

assert_developer_id "$dmg" image
xcrun stapler validate "$dmg"
spctl --assess --type open --context context:primary-signature -vv "$dmg"

hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$mount" -quiet
app="$mount/Bootable.app"
codesign --verify --deep --strict --verbose=2 "$app"
assert_developer_id "$app"
for executable in bootable bootable-helper; do
  assert_developer_id "$app/Contents/MacOS/$executable"
done
xcrun stapler validate "$app"
spctl --assess --type execute -vv "$app"
hdiutil detach "$mount" -quiet

tar -xzf "$archive" -C "$extract"
for executable in bootable bootable-desktop bootable-helper; do
  assert_developer_id "$extract/$executable"
done
echo "macOS packages are Developer ID signed, notarized, and stapled."
