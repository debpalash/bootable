# Media validation

Bootable separates non-destructive boot validation from destructive device tests.

## UEFI smoke test

`scripts/qemu-uefi-smoke.sh` starts QEMU with OVMF, no networking, and the supplied ISO, disk, or
file-backed USB image attached read-only. It sends the optical boot key when testing a CD/DVD image,
waits for firmware and the loader, captures a screenshot, and shuts the VM down.

```bash
scripts/qemu-uefi-smoke.sh --cdrom image.iso /tmp/bootable-uefi.png
scripts/qemu-uefi-smoke.sh --disk disk.img /tmp/bootable-disk-uefi.png
scripts/qemu-uefi-smoke.sh --usb usb.img /tmp/bootable-usb-uefi.png
```

For automation, pass a six-digit expected average RGB value as the fourth argument. The command
fails when the rendered frame differs by more than `BOOTABLE_QEMU_COLOR_TOLERANCE` per channel.
CI builds a tiny deterministic GRUB/UEFI fixture and uses this assertion to prove the VM advanced
into its boot payload rather than merely accepting an attached file:

```bash
scripts/qemu-uefi-fixture.sh /tmp/bootable-uefi-fixture.iso
BOOTABLE_QEMU_WAIT_SECONDS=10 BOOTABLE_QEMU_COLOR_TOLERANCE=64 \
  scripts/qemu-uefi-smoke.sh --cdrom /tmp/bootable-uefi-fixture.iso \
  /tmp/bootable-uefi-fixture.png 0000aa
```

The full virtual-USB assertion creates a larger disposable backing file, attaches only that file as a
temporary `/dev/loopN`, writes and byte-verifies the selected image through Bootable's production raw
writer, detaches it, and presents the same file to QEMU as removable USB storage:

```bash
BOOTABLE_QEMU_WAIT_SECONDS=10 BOOTABLE_QEMU_COLOR_TOLERANCE=64 \
  scripts/qemu-usb-write-uefi-smoke.sh /tmp/bootable-uefi-fixture.iso \
  /tmp/bootable-qemu-usb.png 0000aa
```

Administrator authentication is required only to create, test, and detach that exact temporary loop
device. Normal Bootable discovery continues to exclude loop devices, and the harness refuses any
target path that is not `/dev/loopN`. Set `BOOTABLE_ELEVATE=pkexec` on a desktop host to use its
Polkit prompt instead of `sudo`; CI uses the default `sudo` path.

Set `BOOTABLE_QEMU_WAIT_SECONDS`, `BOOTABLE_QEMU_MEMORY`, `BOOTABLE_OVMF_CODE`, or
`BOOTABLE_OVMF_VARS` when the host needs different timing or firmware paths. A successful process
exit without an expected color proves that OVMF and QEMU accepted the media and produced a frame;
inspect the screenshot to confirm the expected loader. Supplying an expected color turns that
visual check into a machine-readable assertion.

The 2026-08-18 local smoke run reached the Omarchy splash and the Windows optical boot/loader path
from the project test ISOs. The harness does not claim an operating-system installation completed.

## Physical media

Attach physical devices only with deliberate operator review and keep QEMU's read-only option intact. Reading a Linux
block device commonly requires root or membership in the `disk` group. Do not loosen device-node
permissions as a workaround. Bootable's destructive write tests must continue to revalidate a stable,
removable, non-system target immediately before erasure.

`scripts/loop-write-smoke.sh` creates only temporary source/backing files, attaches the backing file to
`/dev/loopN`, and runs the ignored block-I/O integration test as root. The test streams, flushes, and
byte-verifies through the production raw writer. Loop devices remain excluded by normal discovery;
the test-only fixture never changes the removable-drive policy.
