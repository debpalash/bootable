#!/usr/bin/env bash
set -euo pipefail

version="${1:?usage: package-macos.sh VERSION [TARGET] [OUTPUT_DIR]}"
target="${2:-aarch64-apple-darwin}"
output="${3:-dist/macos}"
root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
binary_dir="$root/target/$target/release"
output="$root/$output"

for executable in bootable bootable-desktop bootable-helper; do
  test -x "$binary_dir/$executable" || {
    echo "missing executable: $binary_dir/$executable" >&2
    exit 1
  }
done
mkdir -p "$output"
work="$(mktemp -d "${TMPDIR:-/tmp}/bootable-macos-package.XXXXXX")"
trap 'rm -rf "$work"' EXIT

app="$work/dmg/Bootable.app"
contents="$app/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources"
install -m 0755 "$binary_dir/bootable-desktop" "$contents/MacOS/bootable-desktop"
install -m 0755 "$binary_dir/bootable" "$contents/MacOS/bootable"
install -m 0755 "$binary_dir/bootable-helper" "$contents/MacOS/bootable-helper"

iconset="$work/bootable.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$root/assets/bootable-mark.png" \
    --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double="$((size * 2))"
  sips -z "$double" "$double" "$root/assets/bootable-mark.png" \
    --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$contents/Resources/bootable.icns"

cat > "$contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>Bootable</string>
  <key>CFBundleExecutable</key><string>bootable-desktop</string>
  <key>CFBundleIconFile</key><string>bootable</string>
  <key>CFBundleIdentifier</key><string>app.bootable.Bootable</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>Bootable</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
plutil -lint "$contents/Info.plist"
codesign --force --deep --sign - "$app"

cat > "$work/dmg/Install Bootable Helper.command" <<'EOF'
#!/bin/sh
set -eu
bundle="$(CDPATH= cd -- "$(dirname -- "$0")/Bootable.app/Contents/MacOS" && pwd)"
echo "Bootable installs only its narrow media-write helper with administrator ownership."
sudo install -d -m 0755 /Library/PrivilegedHelperTools
sudo install -m 0755 "$bundle/bootable-helper" /Library/PrivilegedHelperTools/app.bootable.helper
sudo chown root:wheel /Library/PrivilegedHelperTools/app.bootable.helper
echo "Bootable helper installed. You can now launch Bootable.app."
EOF
chmod 0755 "$work/dmg/Install Bootable Helper.command"
ln -s /Applications "$work/dmg/Applications"

dmg="$output/bootable-${version}-aarch64.dmg"
hdiutil create -volname "Bootable $version" -srcfolder "$work/dmg" -ov -format UDZO "$dmg"

archive_stage="$work/archive"
mkdir -p "$archive_stage"
install -m 0755 "$binary_dir/bootable" "$archive_stage/bootable"
install -m 0755 "$binary_dir/bootable-desktop" "$archive_stage/bootable-desktop"
install -m 0755 "$binary_dir/bootable-helper" "$archive_stage/bootable-helper"
install -m 0755 "$root/scripts/install.sh" "$archive_stage/install.sh"
install -m 0644 "$root/assets/bootable-mark.svg" "$archive_stage/bootable.svg"
install -m 0644 "$root/README.md" "$root/LICENSE" "$archive_stage/"
tar -C "$archive_stage" -czf "$output/bootable-${version}-${target}.tar.gz" .

for asset in "$output"/*; do
  case "$asset" in *.sha256) continue ;; esac
  (cd "$output" && shasum -a 256 "$(basename "$asset")" > "$(basename "$asset").sha256")
done
