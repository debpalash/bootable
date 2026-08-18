# Roadmap

## 0.1 — Linux foundation

- Image classification and strategy planning
- Safe `lsblk` device discovery
- Raw stream writing with read-back SHA-256 verification
- Windows GPT/FAT32 media with split-WIM support
- Ratatui inspection/planning/writing interface
- GPUI image, drive, and plan interface

## 0.2 — privilege and test harness

- [x] Polkit/pkexec-authenticated Linux helper with a narrow stdin/stdout JSON protocol
- [x] Cancellation at safe boundaries and resumable UI progress
- [x] Root-only loop-device integration harness using synthetic images without changing discovery policy
- [x] CI assertion for the read-only OVMF/QEMU screenshot harness using a deterministic UEFI fixture
- Signed release artifacts and udev-driven hotplug refresh

## 0.3 — native adapters

- [x] Windows USB-only discovery, stable revalidation, volume detachment, and already-elevated
  `PhysicalDrive` raw writing/verification
- [x] macOS whole-removable discovery, root-disk exclusion, stable revalidation, unmounting, and
  already-elevated `/dev/rdisk` raw writing/verification
- Native privilege prompts and code signing for each platform
- Platform-native Windows FAT32/split-WIM execution

## Later

- [x] Streaming compressed raw/hybrid images (`.xz`, `.gz`, `.zst`, `.bz2`)
- [x] Download with publisher checksum sidecar/manifest verification and truthful fallback states
- OpenPGP/minisign signature verification where publishers provide signatures
- Persistent multi-boot media as a separate, explicit strategy
- BIOS compatibility helpers when an image cannot boot via UEFI
