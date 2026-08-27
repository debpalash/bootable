# Changelog

All notable changes to Bootable are documented here.

## 0.1.0-alpha.3 — 2026-08-27

- Reworked the desktop and terminal interfaces around the same workspace-first flow, shared
  four-step progress model, safety guidance, target eligibility labels, and retry behavior.
- Added explicit keyboard focus and target selection to the TUI, plus responsive layouts for both
  interfaces without changing their shared capability or information hierarchy.
- Restored live DistroWatch catalog loading in both interfaces while retaining the existing cache
  and request limits, and deferred catalog work until discovery is opened.
- Redesigned the website with a compact independent-developer visual system and refreshed the
  download, feature, FAQ, and task-guide content.

## 0.1.0-alpha.2 — 2026-08-18

- Native macOS GPT/MBR FAT32 Windows-installer creation with read-only `hdiutil` source mounting,
  strict whole-disk and partition identity checks, and `diskutil` formatting.
- Pre-erasure Windows boot-tree, answer-file collision, regular-file, and FAT32-size validation.
- Oversized install payload splitting and Windows CA 2023 bootloader support through a fixed,
  preflighted `wimlib-imagex` executable.
- Post-copy UEFI, Windows payload, split-WIM, and FAT32 file-size verification with cancellable
  copy and external-tool execution.

## 0.1.0-alpha.1 — 2026-08-18

Initial public alpha.

- Native GPUI desktop and mouse-friendly Ratatui interfaces backed by one Rust imaging engine.
- Linux removable-media discovery, stable-target validation, authenticated helper writes, and
  byte-range read-back verification.
- Hybrid ISO/IMG and Windows FAT32/split-WIM strategies with reviewed destructive consequences.
- DistroWatch, Omarchy, and Raspberry Pi discovery with verified background downloads.
- Popularity-ranked Arch and Debian quick views and terminal catalog artwork.
- Persistent FIFO download history shared by GUI/TUI, validated HTTP Range resume, retry,
  pause/cancellation, owned-partial cleanup, free-space preflight, and detailed transfer stages.
- Publisher checksum sidecar/manifest association, strongest-supported digest selection, persisted
  verification across resume/restart, mismatch refusal, and truthful HTTPS-only states in both UIs.
- Safe mid-write cancellation shared by GUI/TUI, including privileged-helper forwarding, chunk-level
  cancellation checks, controlled child-process termination, raw-write flushing, and explicit
  incomplete-media guidance.
- Root-owned Linux helper validation and a packaged PolicyKit action; user-writable helpers are never
  executed with administrator privileges.
- Streaming XZ/gzip/Zstandard/bzip2 raw and hybrid images, with asynchronous GUI/TUI inspection,
  expanded-size safety checks, cancellation, and verification of the expanded target bytes.
- A root-only temporary loop-device integration harness that exercises real block I/O without making
  loop devices eligible application targets.
- A deterministic read-only GRUB/OVMF fixture and machine-readable QEMU frame assertion for CI.
- Conservative native Windows USB inventory, stable `PhysicalDrive` revalidation, volume detachment,
  UAC-authenticated raw write/verification, authenticated loopback helper protocol, and protected
  Program Files installer.
- Native Windows GPT/MBR FAT32 installer creation using mounted ISOs and DISM, including oversized
  WIM splitting, the shared Windows 11 options, CA 2023 boot files, pre-erasure source checks, and
  post-copy boot-tree verification.
- Conservative native macOS whole-removable inventory, root-disk exclusion, stable `/dev/rdisk`
  revalidation, unmounting, and already-elevated raw write/backup/verification.
- macOS administrator authentication through a fixed root-owned helper and private Unix-socket
  reviewed-plan/cancel/progress protocol; the GUI/TUI process remains unprivileged.

At this release, native macOS extracted FAT32 creation and platform code signing were unfinished.
