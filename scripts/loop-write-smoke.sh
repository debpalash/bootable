#!/bin/sh
set -eu

for command in cargo losetup truncate sudo; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "Required command is missing: $command" >&2
    exit 1
  }
done

work="$(mktemp -d "${TMPDIR:-/tmp}/bootable-loop.XXXXXX")"
source_image="$work/source.img"
backing_image="$work/target.img"
loop_device=""

cleanup() {
  if [ -n "$loop_device" ]; then
    sudo losetup --detach "$loop_device" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT HUP INT TERM

# A deterministic, non-sparse payload catches short writes and incorrect offsets.
dd if=/dev/zero of="$source_image" bs=1M count=8 status=none
printf 'BOOTABLE-LOOP-SMOKE\n' | dd of="$source_image" conv=notrunc status=none
truncate -s 16M "$backing_image"

echo "Creating a temporary loop device; administrator authentication may be required."
loop_device="$(sudo losetup --find --show "$backing_image")"
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

sudo env \
  "BOOTABLE_LOOP_SOURCE=$source_image" \
  "BOOTABLE_LOOP_DEVICE=$loop_device" \
  "$test_binary" \
  platform::linux::tests::temporary_loop_device_streams_and_verifies_without_relaxing_discovery \
  --exact --ignored --nocapture

echo "Loop-device write and byte verification passed on $loop_device."
