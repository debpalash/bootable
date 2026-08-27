#!/usr/bin/env bash
set -euo pipefail

version="${1:?usage: verify-linux-packages.sh VERSION [OUTPUT_DIR]}"
output="${2:-dist/linux}"
root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
output="$root/$output"

deb="$output/bootable_${version}_amd64.deb"
rpm="$output/bootable-${version}-1.x86_64.rpm"
appimage="$output/bootable-${version}-x86_64.AppImage"
archive="$output/bootable-${version}-x86_64-unknown-linux-gnu.tar.gz"

(cd "$output" && sha256sum -c -- *.sha256)
test "$(dpkg-deb -f "$deb" Version)" = "$version"
deb_contents="$(dpkg-deb -c "$deb")"
for path in ./usr/bin/bootable ./usr/bin/bootable-desktop ./usr/libexec/bootable-helper \
  ./usr/share/applications/app.bootable.Bootable.desktop \
  ./usr/share/polkit-1/actions/app.bootable.write-media.policy; do
  grep -Fq "$path" <<<"$deb_contents"
done
test "$(rpm -qp --queryformat '%{VERSION}' "$rpm")" = "$version"
rpm_contents="$(rpm -qlp "$rpm")"
for path in /usr/bin/bootable /usr/bin/bootable-desktop /usr/libexec/bootable-helper \
  /usr/share/applications/app.bootable.Bootable.desktop \
  /usr/share/polkit-1/actions/app.bootable.write-media.policy; do
  grep -Fxq "$path" <<<"$rpm_contents"
done

extract="$(mktemp -d "${TMPDIR:-/tmp}/bootable-linux-verify.XXXXXX")"
trap 'rm -rf "$extract"' EXIT
(cd "$extract" && "$appimage" --appimage-extract >/dev/null)
for path in AppRun usr/bin/bootable usr/bin/bootable-desktop usr/bin/bootable-helper; do
  test -x "$extract/squashfs-root/$path"
done
test "$("$extract/squashfs-root/usr/bin/bootable" --version)" = "bootable $version"

mkdir "$extract/archive"
tar -xzf "$archive" -C "$extract/archive"
for executable in bootable bootable-desktop bootable-helper install.sh; do
  test -x "$extract/archive/$executable"
done
test "$("$extract/archive/bootable" --version)" = "bootable $version"
"$extract/archive/bootable" --help >/dev/null
