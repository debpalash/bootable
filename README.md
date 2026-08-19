Bootable: Cross-platform boot media writer
===========================================

[![CI](https://img.shields.io/github/actions/workflow/status/debpalash/bootable/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/debpalash/bootable/actions/workflows/ci.yml)
[![Alpha release](https://img.shields.io/github/v/release/debpalash/bootable?include_prereleases&style=flat-square&label=Alpha)](https://github.com/debpalash/bootable/releases)
[![Downloads](https://img.shields.io/github/downloads/debpalash/bootable/total.svg?style=flat-square&label=Downloads)](https://github.com/debpalash/bootable/releases)
[![Contributors](https://img.shields.io/github/contributors/debpalash/bootable.svg?style=flat-square&label=Contributors)](https://github.com/debpalash/bootable/graphs/contributors)
[![License](https://img.shields.io/badge/license-Apache--2.0-5bd7c0.svg?style=flat-square&label=License)](LICENSE)

<p align="center">
  <img src="assets/bootable-logo.svg" width="360" alt="Bootable">
</p>

Bootable writes ISO, IMG, RAW, and compressed disk images to removable USB and SD media.

> [!CAUTION]
> **Bootable 0.1.0-alpha.2 is prerelease software. Writing an image erases the selected device.**
> Verify the physical target and keep backups. Do not use irreplaceable media.

Screenshots
-----------

<img width="1749" height="1948" alt="image" src="https://github.com/user-attachments/assets/18a0ffcf-cedc-403b-b9d3-efb9d32f4bff" />

Features
--------

* Write hybrid Linux and Unix ISOs, IMG files, and RAW disk images
* Stream XZ, gzip, Zstandard, and bzip2 images without a decompressed staging file
* Create GPT or MBR Windows installer media with FAT32 and split WIM payloads
* Apply Windows 11 TPM/Secure Boot bypass, local-account, regional, privacy, BitLocker, QoL,
  CA 2023, SkuSiPolicy, and S Mode options
* Discover Linux distributions through DistroWatch
* Filter popularity-ranked Arch, Debian, Omarchy, Windows, and Raspberry Pi images
* Browse the official Raspberry Pi Imager catalog
* Search, download, pause, resume, cancel, retry, and verify image downloads
* Verify publisher MD5, SHA-1, SHA-256, and SHA-512 checksums when available
* Read back and SHA-256 verify written image bytes
* Detect removable media automatically and exclude fixed and system disks
* Revalidate the target immediately before erasure
* Run destructive bad-block and fake-capacity checks
* Back up removable drives to RAW images
* Review every operation and consequence before writing
* Cancel active writes and mark interrupted media incomplete
* Use the same workflow in the GPUI desktop app and mouse-enabled Ratatui app
* Keep discovery, downloads, inspection, and planning unprivileged
* Elevate only a fixed, protected write helper

Supported media
---------------

| Source | Write method | Verification |
| --- | --- | --- |
| Hybrid Linux/Unix ISO | Raw write | SHA-256 read-back |
| IMG or RAW disk image | Raw write | SHA-256 read-back |
| XZ, gzip, Zstandard, or bzip2 image | Streaming raw write | SHA-256 read-back |
| Windows installer ISO | GPT/MBR + FAT32; split WIM when required | Boot-tree and FAT32 audit |
| Optical-only ISO | Not supported | Refused before writing |

Supported platforms
-------------------

| Platform | Release target | Media support |
| --- | --- | --- |
| Linux | x86-64 | Raw, compressed, Windows installer, backup |
| Windows 10 or later | x86-64 | Raw, compressed, Windows installer |
| macOS | Apple Silicon | Raw, compressed, Windows installer, backup |

Windows installers with a file larger than FAT32's 4 GiB limit require `wimlib` on Linux and
macOS. The Windows build uses DISM. macOS uses `diskutil` and `hdiutil`.

Install
-------

### Linux and macOS

The installer downloads the release archive, verifies its SHA-256 sidecar, and installs under
`~/.local`. Review [`scripts/install.sh`](scripts/install.sh) before piping it into a shell.

Desktop app:

```sh
curl -fsSL https://raw.githubusercontent.com/debpalash/bootable/main/scripts/install.sh | sh -s -- --gui
```

Terminal app:

```sh
curl -fsSL https://raw.githubusercontent.com/debpalash/bootable/main/scripts/install.sh | sh -s -- --tui
```

Both:

```sh
curl -fsSL https://raw.githubusercontent.com/debpalash/bootable/main/scripts/install.sh | sh -s -- --all
```

The Linux installer requests administrator authentication once to install the root-owned helper
and polkit policy. Set `BOOTABLE_SKIP_PRIVILEGED_HELPER=1` for a non-writing installation.

Install `wimlib` for oversized Windows payloads or the CA 2023 option:

```sh
# Debian and Ubuntu
sudo apt install wimtools

# macOS
brew install wimlib
```

### Windows

Download the [Windows release ZIP](https://github.com/debpalash/bootable/releases), verify its
adjacent SHA-256 file, extract it, and run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -Variant All
```

The installer places the protected helper under Program Files. The app requests UAC only when a
reviewed write begins.

Usage
-----

Launch the desktop app:

```sh
bootable-desktop
```

Launch the terminal app:

```sh
bootable
```

Inspect and plan from the command line:

```sh
bootable devices
bootable inspect ~/Downloads/linux.iso
bootable plan ~/Downloads/linux.iso /dev/sdX
```

Catalog and downloads:

```sh
bootable catalog --limit 20
bootable releases cachyos
bootable download cachyos --index 0 --output ~/Downloads/cachyos.iso
bootable pi-images --device pi5-64bit --limit 20
bootable pi-download 0 --output ~/Downloads/raspberry-pi-os.img
```

Checksums and backups:

```sh
bootable checksum image.iso --algorithm sha256
sudo bootable backup /dev/sdX usb-backup.img
```

Direct CLI writes require administrator/root privileges and the confirmation phrase printed by a
fresh plan. The desktop and terminal interfaces use a confirmation dialog instead of phrase entry.

Build
-----

Linux dependencies:

```sh
sudo apt install build-essential pkg-config libxkbcommon-x11-dev \
  xorriso 7zip gdisk parted dosfstools wimtools
```

Build and test:

```sh
cargo build --release --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Binaries:

```text
target/release/bootable
target/release/bootable-desktop
target/release/bootable-helper
```

Documentation
-------------

* [Architecture](docs/architecture.md)
* [Safety model](docs/safety.md)
* [Validation guide](docs/validation.md)
* [GUI/TUI parity contract](docs/ui-parity.md)
* [Rufus feature parity](docs/rufus-parity.md)
* [Roadmap](docs/roadmap.md)
* [Changelog](CHANGELOG.md)
* [Releases](https://github.com/debpalash/bootable/releases)
* [Issues](https://github.com/debpalash/bootable/issues)

Design references
-----------------

Bootable is an original implementation informed by the product behavior of
[Rufus](https://github.com/pbatard/rufus), [WoeUSB-ng](https://github.com/WoeUSB/WoeUSB-ng),
[Etcher](https://github.com/balena-io/etcher), and [PyUSB](https://github.com/pyusb/pyusb).
No source code was copied from those projects.

License
-------

[Apache-2.0](LICENSE)
