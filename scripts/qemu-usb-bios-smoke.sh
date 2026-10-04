#!/bin/sh
# Legacy-BIOS boot-chain smoke test. Needs NO root: it builds a disposable
# image file with an MBR partition at 1 MiB, formats it FAT32 with mkfs.fat,
# copies a *stub* bootmgr (scripts/bios/bootmgr-stub.S, not Microsoft code),
# applies Bootable's BIOS boot sectors through the same bios_boot::install
# routine the Linux writer uses, and boots the result in QEMU/SeaBIOS as a USB
# mass-storage device. Success means: MBR -> active partition -> FAT32 boot
# record -> \BOOTMGR (multi-cluster chain) -> entered at 2000:0000 with the
# documented DL / BPB hidden-sector / drive-number contract.
#
# It does NOT prove that Microsoft's real bootmgr accepts this chain; that needs
# a real Windows ISO and is tracked in docs/legacy-bios.md.
#
# usage: qemu-usb-bios-smoke.sh [sectors-per-cluster]   (default 1)
set -eu

spc="${1:-1}"
for command in cargo qemu-system-x86_64 mkfs.fat sfdisk mcopy mmd truncate as ld objcopy; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "Required command is missing: $command" >&2
    exit 1
  }
done

script_directory="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
repository="$(CDPATH= cd -- "$script_directory/.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/bootable-bios.XXXXXX")"
qemu_pid=""
cleanup() {
  [ -z "$qemu_pid" ] || kill "$qemu_pid" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT HUP INT TERM

image="$work/virtual-usb.img"
as --32 -o "$work/stub.o" "$script_directory/bios/bootmgr-stub.S"
ld -m elf_i386 -Ttext 0 -o "$work/stub.elf" "$work/stub.o"
objcopy -O binary -j .text "$work/stub.elf" "$work/bootmgr"
: > "$work/bcd"
: > "$work/boot.sdi"

# 128 MiB keeps FAT32's 65,525-cluster minimum satisfied up to 2 sectors/cluster.
truncate -s 128M "$image"
# Deliberately NOT bootable: the installer must set the active flag itself.
printf 'label: dos\nlabel-id: 0x1badb002\nstart=2048, type=c\n' | sfdisk --quiet "$image"
mkfs.fat -F 32 -s "$spc" -n WINDOWS --offset 2048 "$image" $((127 * 1024)) >/dev/null
export MTOOLS_SKIP_CHECK=1
mcopy -i "$image@@1M" "$work/bootmgr" ::bootmgr
mmd -i "$image@@1M" ::boot
mcopy -i "$image@@1M" "$work/bcd" ::boot/bcd
mcopy -i "$image@@1M" "$work/boot.sdi" ::boot/boot.sdi

boot() { # $1 = serial log, $2 = seconds
  qemu-system-x86_64 -machine pc -m 64 -display none -no-reboot \
    -serial "file:$1" \
    -drive "if=none,id=usbdisk,format=raw,file=$image,snapshot=on" \
    -device qemu-xhci -device usb-storage,drive=usbdisk,bootindex=0 &
  qemu_pid=$!
  waited=0
  while [ "$waited" -lt "$2" ]; do
    sleep 1
    waited=$((waited + 1))
    grep -q 'tail=' "$1" 2>/dev/null && break
  done
  kill "$qemu_pid" 2>/dev/null || true
  wait "$qemu_pid" 2>/dev/null || true
  qemu_pid=""
}

echo "Control: the same image without Bootable's boot sectors must NOT reach bootmgr."
: > "$work/control.log"
boot "$work/control.log" 10
if grep -q 'BOOTMGR-STUB' "$work/control.log"; then
  echo "FAIL: control image reached the stub; the test cannot discriminate" >&2
  exit 1
fi

echo "Installing the BIOS boot sectors with bootable-core."
(
  cd "$repository"
  BOOTABLE_BIOS_IMAGE="$image" cargo test -p bootable-core --lib -- \
    --ignored --exact bios_boot::tests::install_boot_sectors_on_image_file
)

echo "Booting under SeaBIOS (USB mass storage)."
: > "$work/boot.log"
boot "$work/boot.log" 30
cat "$work/boot.log"
expected='BOOTMGR-STUB dl=80 drv=80 hid=00000800 cs=2000 tail=OK'
if grep -q "$expected" "$work/boot.log"; then
  echo "PASS: MBR -> PBR -> bootmgr stub handoff verified (sectors/cluster=$spc)."
else
  echo "FAIL: expected '$expected'" >&2
  exit 1
fi
