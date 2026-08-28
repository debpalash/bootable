#!/bin/sh
set -eu

usage() {
  echo "usage: qemu-usb-write-uefi-smoke.sh IMAGE [SCREENSHOT.png] [EXPECTED_RGB]" >&2
  exit 2
}

source_image="${1:-}"
screenshot="${2:-qemu-usb-write-uefi-smoke.png}"
expected_rgb="${3:-}"
[ -n "$source_image" ] || usage
[ -r "$source_image" ] || {
  echo "Image is not readable: $source_image" >&2
  exit 1
}

elevation="${BOOTABLE_ELEVATE:-sudo}"
for command in cargo losetup truncate "$elevation" wc; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "Required command is missing: $command" >&2
    exit 1
  }
done

as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  else
    "$elevation" "$@"
  fi
}

script_directory="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/bootable-qemu-usb.XXXXXX")"
target_image="$work/virtual-usb.img"
loop_device=""

cleanup() {
  if [ -n "$loop_device" ]; then
    as_root losetup --detach "$loop_device" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT HUP INT TERM

source_size="$(wc -c < "$source_image" | tr -d ' ')"
case "$source_size" in
  ""|*[!0-9]*)
    echo "Could not determine the image size" >&2
    exit 1
    ;;
esac
[ "$source_size" -gt 0 ] || {
  echo "The image is empty" >&2
  exit 1
}

# Extra capacity makes the virtual target behave like a USB drive larger than
# the selected image. It remains a disposable file under the private temp dir.
target_size=$((source_size + 64 * 1024 * 1024))
truncate -s "$target_size" "$target_image"

echo "Attaching a disposable file-backed target; administrator authentication may be required."
loop_device="$(as_root losetup --find --show "$target_image")"
case "$loop_device" in
  /dev/loop[0-9]*) ;;
  *)
    echo "Refusing unexpected loop-device path: $loop_device" >&2
    exit 1
    ;;
esac

cargo test -p bootable-core --lib --no-run >/dev/null
test_binary="$(
  find target/debug/deps -maxdepth 1 -type f -name 'bootable_core-*' -perm -0100 \
    -printf '%T@ %p\n' | sort -nr | sed -n '1s/^[^ ]* //p'
)"
[ -n "$test_binary" ] && [ -x "$test_binary" ] || {
  echo "Could not locate the bootable-core test binary" >&2
  exit 1
}
case "$test_binary" in
  /*) ;;
  *) test_binary="$(pwd)/$test_binary" ;;
esac

as_root env \
  "BOOTABLE_LOOP_SOURCE=$source_image" \
  "BOOTABLE_LOOP_DEVICE=$loop_device" \
  "$test_binary" \
  platform::linux::tests::temporary_loop_device_streams_and_verifies_without_relaxing_discovery \
  --exact --ignored --nocapture

# QEMU must own the backing file directly, so detach the host loop mapping
# before presenting that same file as removable USB storage to the guest.
as_root losetup --detach "$loop_device"
loop_device=""

echo "Bootable write and verification passed; booting the result as QEMU USB under UEFI."
"$script_directory/qemu-uefi-smoke.sh" --usb "$target_image" "$screenshot" "$expected_rgb"
