#!/usr/bin/env bash
# Developer ID sign, notarize, and staple the packages made by package-macos.sh.
#
# Runs after package-macos.sh and before verify-macos-packages.sh. It rewrites the
# DMG and tar.gz in place and refreshes their .sha256 sidecars.
#
# Required environment (see docs/signing.md):
#   APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD   base64 Developer ID Application .p12
# plus one notarization credential set:
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD     app-specific password, or
#   APPLE_API_KEY_P8, APPLE_API_KEY_ID, APPLE_API_ISSUER_ID   App Store Connect API key
set -euo pipefail

version="${1:?usage: sign-macos.sh VERSION [TARGET] [OUTPUT_DIR]}"
target="${2:-aarch64-apple-darwin}"
output="${3:-dist/macos}"
root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
output="$root/$output"
dmg_name="bootable-${version}-aarch64.dmg"
archive_name="bootable-${version}-${target}.tar.gz"
helper_identifier="app.bootable.helper"

: "${APPLE_CERTIFICATE:?APPLE_CERTIFICATE is required}"
: "${APPLE_CERTIFICATE_PASSWORD:?APPLE_CERTIFICATE_PASSWORD is required}"
test -f "$output/$dmg_name" || { echo "missing $output/$dmg_name" >&2; exit 1; }
test -f "$output/$archive_name" || { echo "missing $output/$archive_name" >&2; exit 1; }

work="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/bootable-macos-sign.XXXXXX")"
keychain="$work/bootable-signing.keychain-db"
mount="$work/mount"
original_keychains=()
cleanup() {
  set +e
  hdiutil detach "$mount" -quiet >/dev/null 2>&1
  if [[ -f "$keychain" ]]; then
    security delete-keychain "$keychain" >/dev/null 2>&1
  fi
  if [[ ${#original_keychains[@]} -gt 0 ]]; then
    security list-keychains -d user -s "${original_keychains[@]}" >/dev/null 2>&1
  fi
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# --- Temporary keychain holding the Developer ID identity -------------------
while IFS= read -r line; do
  line="${line//\"/}"
  line="${line#"${line%%[![:space:]]*}"}"
  if [[ -n "$line" ]]; then original_keychains+=("$line"); fi
done < <(security list-keychains -d user)

keychain_password="$(openssl rand -hex 24)"
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
printf '%s' "$APPLE_CERTIFICATE" | openssl base64 -d -A -out "$work/certificate.p12"
security import "$work/certificate.p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" \
  -T /usr/bin/codesign -T /usr/bin/security >/dev/null
rm -f "$work/certificate.p12"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" \
  "$keychain" >/dev/null
security list-keychains -d user -s "$keychain" ${original_keychains[@]+"${original_keychains[@]}"}

identity="$(security find-identity -v -p codesigning "$keychain" |
  awk '/Developer ID Application/ { print $2; exit }')"
if [[ -z "$identity" ]]; then
  echo "No valid 'Developer ID Application' identity found in APPLE_CERTIFICATE." >&2
  exit 1
fi
echo "Signing with a Developer ID Application identity."

# --- Notarization credentials -----------------------------------------------
notary_args=()
if [[ -n "${APPLE_API_KEY_P8:-}" ]]; then
  printf '%s' "$APPLE_API_KEY_P8" | openssl base64 -d -A -out "$work/AuthKey.p8"
  chmod 0600 "$work/AuthKey.p8"
  notary_args=(--key "$work/AuthKey.p8" --key-id "${APPLE_API_KEY_ID:?}" --issuer "${APPLE_API_ISSUER_ID:?}")
else
  notary_args=(--apple-id "${APPLE_ID:?}" --team-id "${APPLE_TEAM_ID:?}" --password "${APPLE_APP_PASSWORD:?}")
fi

sign() {
  codesign --force --timestamp --options runtime --keychain "$keychain" --sign "$identity" "$@"
}

json_field() {
  python3 -c 'import json, sys; print(json.load(sys.stdin).get(sys.argv[1], ""))' "$1"
}

# Submit a file to the notary service and require an Accepted verdict.
notarize() {
  local file="$1" result status id
  result="$(xcrun notarytool submit "$file" "${notary_args[@]}" --wait --timeout 45m \
    --output-format json)" || {
    echo "notarytool submit failed for $(basename "$file")." >&2
    echo "$result" >&2
    exit 1
  }
  status="$(json_field status <<<"$result")"
  id="$(json_field id <<<"$result")"
  if [[ "$status" != "Accepted" ]]; then
    echo "Notarization of $(basename "$file") finished with status: ${status:-unknown} (submission $id)." >&2
    [[ -n "$id" ]] && xcrun notarytool log "$id" "${notary_args[@]}" >&2 || true
    exit 1
  fi
  echo "Notarized $(basename "$file") (submission $id)."
}

# --- Stage the app and the bare binaries ------------------------------------
mkdir -p "$mount" "$work/dmg" "$work/cli" "$work/submit/cli"
hdiutil attach "$output/$dmg_name" -readonly -nobrowse -mountpoint "$mount" -quiet
ditto "$mount" "$work/dmg"
hdiutil detach "$mount" -quiet
rm -rf "$work/dmg/.fseventsd" "$work/dmg/.Trashes" "$work/dmg/.DS_Store"
app="$work/dmg/Bootable.app"
test -d "$app"

# Nested executables first, then the bundle (its main executable is sealed by it).
# The privileged helper is signed with the identifier it is installed under.
sign --identifier "$helper_identifier" "$app/Contents/MacOS/bootable-helper"
sign "$app/Contents/MacOS/bootable"
sign "$app"

tar -xzf "$output/$archive_name" -C "$work/cli"
sign --identifier "$helper_identifier" "$work/cli/bootable-helper"
sign "$work/cli/bootable"
sign "$work/cli/bootable-desktop"
for executable in bootable bootable-desktop bootable-helper; do
  cp -p "$work/cli/$executable" "$work/submit/cli/$executable"
done

# --- Notarize app and bare binaries, then staple the app ---------------------
ditto "$app" "$work/submit/Bootable.app"
ditto -c -k --sequesterRsrc "$work/submit" "$work/submit.zip"
notarize "$work/submit.zip"
xcrun stapler staple "$app"
xcrun stapler validate "$app"

# --- Rebuild, sign, notarize, and staple the DMG -----------------------------
new_dmg="$work/$dmg_name"
hdiutil create -volname "Bootable $version" -srcfolder "$work/dmg" -ov -format UDZO "$new_dmg"
sign "$new_dmg"
notarize "$new_dmg"
xcrun stapler staple "$new_dmg"
xcrun stapler validate "$new_dmg"

# --- Repack the archive and refresh checksums --------------------------------
new_archive="$work/$archive_name"
tar -C "$work/cli" -czf "$new_archive" .
mv "$new_dmg" "$output/$dmg_name"
mv "$new_archive" "$output/$archive_name"
for asset in "$dmg_name" "$archive_name"; do
  (cd "$output" && shasum -a 256 "$asset" > "$asset.sha256")
done
echo "Signed, notarized, and stapled macOS packages written to $output."
