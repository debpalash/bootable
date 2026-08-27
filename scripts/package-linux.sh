#!/usr/bin/env bash
set -euo pipefail

version="${1:?usage: package-linux.sh VERSION [TARGET] [OUTPUT_DIR]}"
target="${2:-x86_64-unknown-linux-gnu}"
output="${3:-dist/linux}"
root="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
binary_dir="$root/target/$target/release"
linuxdeploy="${LINUXDEPLOY:-linuxdeploy-x86_64.AppImage}"
appimagetool="${APPIMAGETOOL:-appimagetool-x86_64.AppImage}"

for executable in bootable bootable-desktop bootable-helper; do
  test -x "$binary_dir/$executable" || {
    echo "missing executable: $binary_dir/$executable" >&2
    exit 1
  }
done
command -v dpkg-deb >/dev/null
command -v rpmbuild >/dev/null
test -x "$linuxdeploy"
test -x "$appimagetool"

output="$root/$output"
mkdir -p "$output"
work="$(mktemp -d "${TMPDIR:-/tmp}/bootable-linux-package.XXXXXX")"
trap 'rm -rf "$work"' EXIT

install_payload() {
  local destination="$1"
  install -D -m 0755 "$binary_dir/bootable" "$destination/usr/bin/bootable"
  install -D -m 0755 "$binary_dir/bootable-desktop" "$destination/usr/bin/bootable-desktop"
  install -D -m 0755 "$binary_dir/bootable-helper" "$destination/usr/libexec/bootable-helper"
  install -D -m 0644 "$root/assets/bootable-mark.svg" \
    "$destination/usr/share/icons/hicolor/scalable/apps/bootable.svg"
  install -D -m 0644 "$root/assets/bootable-mark.png" \
    "$destination/usr/share/icons/hicolor/1024x1024/apps/bootable.png"
  install -D -m 0644 "$root/packaging/app.bootable.write-media.policy" \
    "$destination/usr/share/polkit-1/actions/app.bootable.write-media.policy"
  install -D -m 0644 "$root/README.md" "$destination/usr/share/doc/bootable/README.md"
  install -D -m 0644 "$root/LICENSE" "$destination/usr/share/doc/bootable/LICENSE"
  sed 's|@EXEC@|/usr/bin/bootable-desktop|g' \
    "$root/packaging/app.bootable.Bootable.desktop" \
    > "$destination/usr/share/applications/app.bootable.Bootable.desktop"
  chmod 0644 "$destination/usr/share/applications/app.bootable.Bootable.desktop"
}

deb_root="$work/deb"
install_payload "$deb_root"
mkdir -p "$deb_root/DEBIAN"
installed_size="$(du -sk "$deb_root/usr" | awk '{print $1}')"
cat > "$deb_root/DEBIAN/control" <<EOF
Package: bootable
Version: $version
Section: utils
Priority: optional
Architecture: amd64
Installed-Size: $installed_size
Maintainer: Bootable contributors <noreply@github.com>
Homepage: https://github.com/debpalash/bootable
Depends: libc6, libfontconfig1, libxkbcommon-x11-0, polkitd | policykit-1
Description: Safety-first boot media writer
 Bootable provides matching desktop and terminal interfaces for inspecting,
 writing, and verifying bootable images on removable media.
EOF
deb_asset="$output/bootable_${version}_amd64.deb"
dpkg-deb --build --root-owner-group "$deb_root" "$deb_asset"

rpm_top="$work/rpmbuild"
mkdir -p "$rpm_top"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
install -m 0755 "$binary_dir/bootable" "$rpm_top/SOURCES/bootable"
install -m 0755 "$binary_dir/bootable-desktop" "$rpm_top/SOURCES/bootable-desktop"
install -m 0755 "$binary_dir/bootable-helper" "$rpm_top/SOURCES/bootable-helper"
install -m 0644 "$root/assets/bootable-mark.svg" "$rpm_top/SOURCES/bootable.svg"
install -m 0644 "$root/assets/bootable-mark.png" "$rpm_top/SOURCES/bootable.png"
install -m 0644 "$root/packaging/app.bootable.write-media.policy" "$rpm_top/SOURCES/app.bootable.write-media.policy"
sed 's|@EXEC@|/usr/bin/bootable-desktop|g' \
  "$root/packaging/app.bootable.Bootable.desktop" \
  > "$rpm_top/SOURCES/app.bootable.Bootable.desktop"
install -m 0644 "$root/README.md" "$rpm_top/SOURCES/README.md"
install -m 0644 "$root/LICENSE" "$rpm_top/SOURCES/LICENSE"
cat > "$rpm_top/SPECS/bootable.spec" <<EOF
Name: bootable
Version: $version
Release: 1%{?dist}
Summary: Safety-first boot media writer
License: Apache-2.0
URL: https://github.com/debpalash/bootable
Requires: fontconfig, libxkbcommon-x11, polkit

%description
Bootable provides matching desktop and terminal interfaces for inspecting,
writing, and verifying bootable images on removable media.

%prep

%build

%install
install -Dpm0755 %{_sourcedir}/bootable %{buildroot}%{_bindir}/bootable
install -Dpm0755 %{_sourcedir}/bootable-desktop %{buildroot}%{_bindir}/bootable-desktop
install -Dpm0755 %{_sourcedir}/bootable-helper %{buildroot}/usr/libexec/bootable-helper
install -Dpm0644 %{_sourcedir}/bootable.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/bootable.svg
install -Dpm0644 %{_sourcedir}/bootable.png %{buildroot}%{_datadir}/icons/hicolor/1024x1024/apps/bootable.png
install -Dpm0644 %{_sourcedir}/app.bootable.Bootable.desktop %{buildroot}%{_datadir}/applications/app.bootable.Bootable.desktop
install -Dpm0644 %{_sourcedir}/app.bootable.write-media.policy %{buildroot}%{_datadir}/polkit-1/actions/app.bootable.write-media.policy
install -Dpm0644 %{_sourcedir}/README.md %{buildroot}%{_docdir}/bootable/README.md
install -Dpm0644 %{_sourcedir}/LICENSE %{buildroot}%{_licensedir}/bootable/LICENSE

%files
%{_bindir}/bootable
%{_bindir}/bootable-desktop
/usr/libexec/bootable-helper
%{_datadir}/applications/app.bootable.Bootable.desktop
%{_datadir}/icons/hicolor/scalable/apps/bootable.svg
%{_datadir}/icons/hicolor/1024x1024/apps/bootable.png
%{_datadir}/polkit-1/actions/app.bootable.write-media.policy
%doc %{_docdir}/bootable/README.md
%license %{_licensedir}/bootable/LICENSE

%changelog
* Thu Aug 27 2026 Bootable contributors <noreply@github.com> - $version-1
- Native installer release.
EOF
rpmbuild --define "_topdir $rpm_top" -bb "$rpm_top/SPECS/bootable.spec"
rpm_asset="$(find "$rpm_top/RPMS" -type f -name '*.rpm' -print -quit)"
test -n "$rpm_asset"
cp "$rpm_asset" "$output/bootable-${version}-1.x86_64.rpm"

appdir="$work/Bootable.AppDir"
install_payload "$appdir"
sed 's|@EXEC@|bootable-desktop|g' \
  "$root/packaging/app.bootable.Bootable.desktop" \
  > "$appdir/usr/share/applications/app.bootable.Bootable.desktop"
# The adjacent helper lets the runtime retain its fixed-name fallback inside the AppImage.
install -m 0755 "$binary_dir/bootable-helper" "$appdir/usr/bin/bootable-helper"
"$linuxdeploy" \
  --appdir "$appdir" \
  --executable "$appdir/usr/bin/bootable-desktop" \
  --executable "$appdir/usr/bin/bootable" \
  --executable "$appdir/usr/bin/bootable-helper" \
  --desktop-file "$appdir/usr/share/applications/app.bootable.Bootable.desktop" \
  --icon-file "$appdir/usr/share/icons/hicolor/1024x1024/apps/bootable.png"
cat > "$appdir/AppRun" <<'EOF'
#!/bin/sh
set -eu
appdir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
if [ "${1:-}" = "--tui" ]; then
  shift
  exec "$appdir/usr/bin/bootable" "$@"
fi
exec "$appdir/usr/bin/bootable-desktop" "$@"
EOF
chmod 0755 "$appdir/AppRun"
ARCH=x86_64 "$appimagetool" "$appdir" "$output/bootable-${version}-x86_64.AppImage"

archive_stage="$work/archive"
mkdir -p "$archive_stage"
install -m 0755 "$binary_dir/bootable" "$archive_stage/bootable"
install -m 0755 "$binary_dir/bootable-desktop" "$archive_stage/bootable-desktop"
install -m 0755 "$binary_dir/bootable-helper" "$archive_stage/bootable-helper"
install -m 0755 "$root/scripts/install.sh" "$archive_stage/install.sh"
install -m 0644 "$root/assets/bootable-mark.svg" "$archive_stage/bootable.svg"
install -m 0644 "$root/packaging/app.bootable.Bootable.desktop" "$archive_stage/app.bootable.Bootable.desktop"
install -m 0644 "$root/packaging/app.bootable.write-media.policy" "$archive_stage/app.bootable.write-media.policy"
install -m 0644 "$root/README.md" "$root/LICENSE" "$archive_stage/"
tar -C "$archive_stage" -czf "$output/bootable-${version}-${target}.tar.gz" .

for asset in "$output"/*; do
  case "$asset" in *.sha256) continue ;; esac
  (cd "$output" && sha256sum "$(basename "$asset")" > "$(basename "$asset").sha256")
done
