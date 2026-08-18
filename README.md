Bootable: Cross-platform boot media writer
===========================================

[![CI](https://img.shields.io/github/actions/workflow/status/debpalash/bootable/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/debpalash/bootable/actions/workflows/ci.yml)
[![Alpha](https://img.shields.io/github/v/release/debpalash/bootable?include_prereleases&style=flat-square&label=Alpha)](https://github.com/debpalash/bootable/releases)
[![Downloads](https://img.shields.io/github/downloads/debpalash/bootable/total.svg?style=flat-square&label=Downloads)](https://github.com/debpalash/bootable/releases)
[![License](https://img.shields.io/badge/license-Apache--2.0-5bd7c0.svg?style=flat-square)](LICENSE)

<p align="center"><img src="assets/bootable-logo.svg" width="360" alt="Bootable"></p>

Write ISO, IMG, RAW, and compressed disk images to removable USB and SD media.

> [!CAUTION]
> **0.1.0-alpha.2 is prerelease software. Writing erases the selected device.**
> Check the physical target and keep backups.

Screenshot
----------

<img width="1749" height="1948" alt="Bootable desktop app" src="https://github.com/user-attachments/assets/18a0ffcf-cedc-403b-b9d3-efb9d32f4bff">

Download
--------

### Linux and macOS

```sh
# Desktop
curl -fsSL https://raw.githubusercontent.com/debpalash/bootable/main/scripts/install.sh | sh -s -- --gui

# TUI
curl -fsSL https://raw.githubusercontent.com/debpalash/bootable/main/scripts/install.sh | sh -s -- --tui

# Both
curl -fsSL https://raw.githubusercontent.com/debpalash/bootable/main/scripts/install.sh | sh -s -- --all
```

The installer verifies the release checksum and installs the protected helper. Set
`BOOTABLE_SKIP_PRIVILEGED_HELPER=1` for a non-writing install. Oversized Windows payloads and CA
2023 require `wimtools` on Linux or `wimlib` on macOS.

### Windows

Download and verify the [release ZIP](https://github.com/debpalash/bootable/releases), extract it,
then run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -Variant All
```

Features
--------

* Raw-write Linux/Unix ISOs, IMG, RAW, XZ, gzip, Zstandard, and bzip2 images
* Build GPT/MBR FAT32 Windows media; split oversized WIM payloads
* Apply Windows 11 hardware, account, regional, privacy, BitLocker, QoL, CA 2023,
  SkuSiPolicy, and S Mode options
* Discover DistroWatch, Omarchy, and Raspberry Pi images
* Search, queue, pause, resume, cancel, retry, and checksum downloads
* Verify MD5, SHA-1, SHA-256, SHA-512, written bytes, and Windows boot trees
* Auto-detect removable media; exclude fixed and system disks
* Revalidate the target before erasure; review consequences before writing
* Cancel writes; mark interrupted media incomplete
* Run bad-block/fake-capacity checks and RAW backups
* Use a native GPUI desktop app or mouse-enabled Ratatui app
* Elevate only a protected write helper

Comparison
----------

| | Bootable alpha | [Rufus](https://github.com/pbatard/rufus#features) | [Etcher](https://github.com/balena-io/etcher#etcher) |
| --- | --- | --- | --- |
| Platforms | Linux, Windows, macOS | Windows | Linux, Windows, macOS |
| UI | Native GUI, mouse TUI, CLI | Native Windows GUI | Electron GUI |
| Windows media | FAT32, split WIM, Windows 11/OOBE options | UEFI:NTFS, Windows To Go, OOBE, broader format support | Raw-image writing |
| Discovery | DistroWatch, Omarchy, Raspberry Pi | Microsoft Windows, UEFI Shell | File, URL, drive clone |
| Verification | Checksums, read-back, boot-tree audit | Checksums, bad blocks, runtime UEFI validation | Written-data validation |
| Privilege model | Protected narrow helper | Whole portable app requests administrator access | Packaged `etcher-util` sidecar |
| License | Apache-2.0 | GPL-3.0 | Apache-2.0 |

Sources: [Rufus manifest](https://github.com/pbatard/rufus/blob/master/src/rufus.manifest) ·
[Etcher sidecar](https://github.com/balena-io/etcher/blob/master/forge.sidecar.ts) ·
[Rufus parity tracker](docs/rufus-parity.md)

Executables
-----------

| File | Role | Privilege |
| --- | --- | --- |
| `bootable` | TUI and CLI | Normal user |
| `bootable-desktop` | GPUI desktop | Normal user |
| `bootable-helper` | Revalidate, erase, write, verify, back up | Root/admin |

The GUI and TUI could share one larger binary, but separate builds keep GPUI out of terminal-only
installs. Both use `bootable-core`. The helper stays separate so networking, catalog, decoding,
terminal, and graphics code never enter the root/admin process. Users do not launch it directly.

Rufus chooses one elevated Windows executable. Etcher also packages a separate privileged sidecar.
A future unified Bootable launcher would still keep `bootable-helper` separate.

Run
---

```sh
bootable-desktop       # GUI
bootable               # TUI
bootable --help        # CLI
```

Release targets
---------------

| Platform | Architecture | Media |
| --- | --- | --- |
| Linux | x86-64 | Raw, compressed, Windows installer, backup |
| Windows 10+ | x86-64 | Raw, compressed, Windows installer |
| macOS | Apple Silicon | Raw, compressed, Windows installer, backup |

Build
-----

```sh
sudo apt install build-essential pkg-config libxkbcommon-x11-dev \
  xorriso 7zip gdisk parted dosfstools wimtools

cargo build --release --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Documentation
-------------

[Architecture](docs/architecture.md) · [Safety](docs/safety.md) ·
[Validation](docs/validation.md) · [GUI/TUI parity](docs/ui-parity.md) ·
[Roadmap](docs/roadmap.md) · [Changelog](CHANGELOG.md) ·
[Releases](https://github.com/debpalash/bootable/releases) ·
[Issues](https://github.com/debpalash/bootable/issues)

References
----------

Original Apache-2.0 implementation informed by [Rufus](https://github.com/pbatard/rufus),
[WoeUSB-ng](https://github.com/WoeUSB/WoeUSB-ng), [Etcher](https://github.com/balena-io/etcher),
and [PyUSB](https://github.com/pyusb/pyusb). No source code was copied.

License
-------

[Apache-2.0](LICENSE)
