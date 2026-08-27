#!/bin/sh
set -eu

REPOSITORY="${BOOTABLE_REPOSITORY:-debpalash/bootable}"
VERSION="${BOOTABLE_VERSION:-0.1.1}"
VARIANT="${1:---gui}"
INSTALL_ROOT="${BOOTABLE_INSTALL_ROOT:-${HOME}/.local}"

case "$VARIANT" in
  --gui|--tui|--all) ;;
  *)
    echo "usage: install.sh [--gui|--tui|--all]" >&2
    exit 2
    ;;
esac

case "$(uname -s)" in
  Linux) platform="unknown-linux-gnu" ;;
  Darwin) platform="apple-darwin" ;;
  *)
    echo "This installer supports Linux and macOS. Use the Windows ZIP release on Windows." >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  x86_64|amd64) architecture="x86_64" ;;
  arm64|aarch64) architecture="aarch64" ;;
  *)
    echo "Unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

asset="bootable-${VERSION}-${architecture}-${platform}.tar.gz"
base="https://github.com/${REPOSITORY}/releases/download/v${VERSION}"
temporary="$(mktemp -d "${TMPDIR:-/tmp}/bootable-install.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

echo "Bootable ${VERSION} can erase removable drives after explicit review and confirmation."
echo "Downloading ${asset}…"
curl -fL --proto '=https' --tlsv1.2 "${base}/${asset}" -o "${temporary}/${asset}"
curl -fL --proto '=https' --tlsv1.2 "${base}/${asset}.sha256" -o "${temporary}/${asset}.sha256"

(
  cd "$temporary"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "${asset}.sha256"
  else
    shasum -a 256 -c "${asset}.sha256"
  fi
  tar -xzf "$asset"
)

mkdir -p "${INSTALL_ROOT}/bin"
case "$VARIANT" in
  --gui)
    install -m 0755 "${temporary}/bootable-desktop" "${INSTALL_ROOT}/bin/bootable-desktop"
    ;;
  --tui)
    install -m 0755 "${temporary}/bootable" "${INSTALL_ROOT}/bin/bootable"
    ;;
  --all)
    install -m 0755 "${temporary}/bootable" "${INSTALL_ROOT}/bin/bootable"
    install -m 0755 "${temporary}/bootable-desktop" "${INSTALL_ROOT}/bin/bootable-desktop"
    ;;
esac

if [ "$platform" = "unknown-linux-gnu" ] && [ "$VARIANT" != "--tui" ]; then
  applications="${HOME}/.local/share/applications"
  icons="${HOME}/.local/share/icons/hicolor/scalable/apps"
  mkdir -p "$applications" "$icons"
  install -m 0644 "${temporary}/bootable.svg" "${icons}/bootable.svg"
  sed "s|@EXEC@|${INSTALL_ROOT}/bin/bootable-desktop|g" \
    "${temporary}/app.bootable.Bootable.desktop" \
    > "${applications}/app.bootable.Bootable.desktop"
fi

if [ "$platform" = "unknown-linux-gnu" ] && [ -f "${temporary}/bootable-helper" ]; then
  if [ "${BOOTABLE_SKIP_PRIVILEGED_HELPER:-0}" = "1" ]; then
    echo "Skipping the privileged helper; discovery and downloads work, but writing will be unavailable."
  else
    echo "Installing the narrow, root-owned write helper (administrator authentication required)…"
    if [ "$(id -u)" -eq 0 ]; then
      install -d -m 0755 /usr/libexec /usr/share/polkit-1/actions
      install -m 0755 "${temporary}/bootable-helper" /usr/libexec/bootable-helper
      install -m 0644 "${temporary}/app.bootable.write-media.policy" \
        /usr/share/polkit-1/actions/app.bootable.write-media.policy
    else
      sudo install -d -m 0755 /usr/libexec /usr/share/polkit-1/actions
      sudo install -m 0755 "${temporary}/bootable-helper" /usr/libexec/bootable-helper
      sudo install -m 0644 "${temporary}/app.bootable.write-media.policy" \
        /usr/share/polkit-1/actions/app.bootable.write-media.policy
    fi
  fi
fi

if [ "$platform" = "apple-darwin" ] && [ -f "${temporary}/bootable-helper" ]; then
  if [ "${BOOTABLE_SKIP_PRIVILEGED_HELPER:-0}" = "1" ]; then
    echo "Skipping the privileged helper; discovery and downloads work, but writing will require a separately installed helper."
  else
    echo "Installing the narrow, root-owned macOS write helper (administrator authentication required)…"
    if [ "$(id -u)" -eq 0 ]; then
      install -d -m 0755 /Library/PrivilegedHelperTools
      install -m 0755 "${temporary}/bootable-helper" \
        /Library/PrivilegedHelperTools/app.bootable.helper
      chown root:wheel /Library/PrivilegedHelperTools/app.bootable.helper
    else
      sudo install -d -m 0755 /Library/PrivilegedHelperTools
      sudo install -m 0755 "${temporary}/bootable-helper" \
        /Library/PrivilegedHelperTools/app.bootable.helper
      sudo chown root:wheel /Library/PrivilegedHelperTools/app.bootable.helper
    fi
  fi
fi

echo "Installed ${VARIANT#--} variant under ${INSTALL_ROOT}."
case ":${PATH}:" in
  *":${INSTALL_ROOT}/bin:"*) ;;
  *) echo "Add ${INSTALL_ROOT}/bin to PATH before launching the TUI." ;;
esac
