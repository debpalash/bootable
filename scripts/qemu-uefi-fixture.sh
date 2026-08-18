#!/bin/sh
set -eu

destination="${1:-qemu-uefi-fixture.iso}"
for command in grub-mkrescue mformat xorriso; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "Required command is missing: $command" >&2
    exit 1
  }
done

work="$(mktemp -d "${TMPDIR:-/tmp}/bootable-uefi-fixture.XXXXXX")"
cleanup() {
  rm -rf "$work"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$work/boot/grub" "$(dirname "$destination")"
printf '%s\n' \
  'set timeout=0' \
  'set timeout_style=hidden' \
  'insmod all_video' \
  'insmod gfxterm' \
  'set gfxmode=800x600' \
  'set gfxpayload=keep' \
  'terminal_output gfxterm' \
  'background_color blue' \
  'set color_normal=white/blue' \
  'set color_highlight=white/blue' \
  'clear' \
  'echo "BOOTABLE UEFI SMOKE PASS"' \
  'sleep 30' \
  > "$work/boot/grub/grub.cfg"

grub-mkrescue -o "$destination" "$work"
echo "Deterministic UEFI fixture: $destination"
