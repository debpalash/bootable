# Rufus 4.15 feature parity

Bootable treats parity as working, verified behavior in both the Ratatui and GPUI interfaces.
A visible but non-functional control does not count. Destructive features must also pass the same
stable-device identity and system-disk protections as normal writes.

This inventory was checked against the official Rufus 4.15 feature list on 2026-08-18. Bootable
is an original cross-platform implementation; parity means equivalent outcomes, not copied code.

| Capability | Core | TUI | GPUI | Status |
| --- | --- | --- | --- | --- |
| Automatic USB/device refresh | stable-ID `lsblk` scan | 1-second refresh | 1-second refresh | Implemented on Linux |
| Safe removable-drive filtering | yes | removable media only | removable media only | Implemented on Linux |
| Image and folder dialogs | shared paths | native dialogs + clickable buttons | native dialogs + clickable buttons | Implemented |
| Mouse control | n/a | rows, wheel, actions | all controls | Implemented |
| MD5/SHA-1/SHA-256/SHA-512 | streaming digest API | CLI + mouse/keyboard picker | mouse picker | Implemented |
| Hybrid ISO/IMG writing | raw write | consequence modal + polkit helper + live write | consequence modal + polkit helper + live write | Implemented on Linux |
| Raw write verification | byte-range SHA-256 | phase, speed, ETA, verification | phase, speed, ETA, verification | Implemented on Linux |
| Windows installer creation | GPT/FAT32 + split WIM | consequence modal + narrow helper + live write | consequence modal + narrow helper + live write | Implemented on Linux and Windows; macOS pending |
| Windows 11 TPM/Secure Boot/RAM bypass | guarded answer file | flag + clickable toggle | clickable toggle | Implemented |
| Runtime UEFI boot validation | reproducible read-only QEMU/OVMF harness + RGB frame assertion | same script | same script | Implemented in CI with a deterministic UEFI fixture |
| Bad-block/fake-drive test | 1/2/4 destructive patterns | flag + clickable cycle | clickable cycle | Implemented on Linux |
| Partition scheme and target firmware choices | GPT or MBR + UEFI | clickable cycle | native select box | Partial: legacy BIOS remains |
| FAT/FAT32/NTFS/UDF/exFAT/ext formatting | Windows FAT32 only | automatic only | automatic only | Planned |
| Linux persistence | not implemented | not implemented | not implemented | Planned |
| Windows To Go | not implemented | not implemented | not implemented | Planned |
| Windows OOBE experience | validated answer-file generator | ten mouse/keyboard choices | ten checkboxes | Implemented: named account, regional clone, requirements, account, privacy, BitLocker, QoL, CA 2023, SkuSiPolicy, S Mode |
| Windows 11 QoL policies | specialize + first-logon policies | clickable toggle | checkbox | Implemented |
| Windows CA 2023 bootloaders | extracts `_EX` files from `boot.wim` | clickable toggle | checkbox | Implemented for compatible images |
| SkuSiPolicy revocations | guarded first-logon command | clickable toggle | checkbox | Implemented |
| Force S Mode | offline-servicing setting | clickable toggle | checkbox | Implemented, expert option |
| Silent unattended installation | deliberately unavailable | coverage shown, unavailable | coverage shown, unavailable | Requires a separate high-friction target-disk safety design |
| Compressed images | streaming XZ/gzip/Zstandard/bzip2 with expanded-size preflight | async inspection + same write plan | async inspection + same write plan | Implemented for raw disk images and hybrid ISOs |
| DOS/FreeDOS media | not implemented | not implemented | not implemented | Planned |
| Drive-to-image backup (DD/VHD/VHDX/FFU) | atomic raw IMG/RAW/DD backup | CLI + save dialog | background backup + save dialog | Partial: VHD/VHDX/FFU remain |
| Official ISO/UEFI Shell downloads | not implemented | not implemented | not implemented | Planned |
| Publisher download integrity | checksum sidecars/manifests, strongest digest, atomic mismatch refusal | integrity label + persistent result | integrity label + persistent result | Implemented for MD5/SHA-1/SHA-256/SHA-512; signatures planned |
| Native Windows/macOS raw adapters | USB/whole-removable discovery, stable refresh, detach/unmount, raw write/backup/verify | same core path | same core path | Raw adapters and narrow-helper authorization implemented; Windows FAT32 implemented, macOS pending |
| Localization | not implemented | not implemented | not implemented | Planned |

## Delivery order

1. Formatting and in-app UEFI validation.
2. Windows To Go.
3. Linux persistence, BIOS helpers, and DOS media.
4. VHD/VHDX/FFU backup, verified downloads, and localization.
5. Native macOS Windows-media execution and signed platform installers.

Every capability is added to the core first. TUI and GPUI controls ship together and consume the
same serialized option types so plans cannot differ by frontend.

## Interface structure

Both frontends use the same task hierarchy:

1. Choose a source image.
2. Choose a removable target.
3. Review the generated strategy and safety plan.
4. Expand **Advanced** only for Windows setup, media testing, checksum, and browsing tools.

Future parity features join the advanced groups by task: **Boot**, **Format**, **Windows setup**,
**Persistence**, **Validation**, and **Utilities**. They do not enlarge the basic three-step flow.
