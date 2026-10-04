#!/bin/sh
# Regenerates mbr.bin and pbr_fat32.bin from their GNU as sources. The binaries
# are committed so building Bootable needs no assembler; unit tests pin their
# layout. Requires GNU binutils (as, ld, objcopy).
set -eu
cd "$(dirname "$0")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
for name in mbr pbr_fat32; do
  as --32 -o "$work/$name.o" "$name.S"
  ld -m elf_i386 -Ttext 0x7c00 -o "$work/$name.elf" "$work/$name.o"
  objcopy -O binary -j .text "$work/$name.elf" "$name.bin"
done
wc -c mbr.bin pbr_fat32.bin
