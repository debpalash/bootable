<p align="center"><a href="https://bootable.palash.dev"><img src="assets/bootable-logo.svg" width="360" alt="Bootable"></a></p>

<p align="center"><strong>Create and verify bootable USB and SD drives.</strong></p>

<p align="center">
  <a href="https://github.com/debpalash/bootable/releases/download/v0.1.0/bootable-0.1.0-x86_64-unknown-linux-gnu.tar.gz"><strong>Linux</strong></a> ·
  <a href="https://github.com/debpalash/bootable/releases/download/v0.1.0/bootable-0.1.0-x86_64-pc-windows-msvc.zip"><strong>Windows</strong></a> ·
  <a href="https://github.com/debpalash/bootable/releases/download/v0.1.0/bootable-0.1.0-aarch64-apple-darwin.tar.gz"><strong>macOS</strong></a> ·
  <a href="https://bootable.palash.dev/download.html">All downloads and checksums</a>
</p>

Bootable finds, downloads, writes, and verifies operating-system images. Use the desktop app or TUI on Linux, Windows, and macOS.

Major features
--------------

- Browse DistroWatch, Omarchy, and Raspberry Pi images without leaving the app.
- Write ISO, IMG, RAW, XZ, gzip, Zstandard, and bzip2 files.
- Create FAT32 Windows installation media, including split WIM files.
- Verify publisher checksums and read back every written byte.
- Reject system disks and require explicit review before erasing a removable drive.
- Follow the same Source → Target → Review & write flow in the GUI and TUI.

GUI and TUI
-----------

Both interfaces keep the same three steps visible:

**Source → Target → Review & write**

Discovery opens only when needed. Image-specific setup appears after source inspection. Review stays
locked until you select an eligible removable drive. Bootable never selects a drive automatically.

<table>
  <tr>
    <th>Desktop GUI</th>
  </tr>
  <tr>
    <td><img alt="Bootable desktop GUI showing image discovery and removable-drive setup" src="docs/screenshots/gui-demo.gif"></td>
  </tr>
  <tr>
    <td align="center">Pointer and keyboard</td>
  </tr>
  <tr>
    <th>Terminal TUI</th>
  </tr>
  <tr>
    <td><img alt="Bootable TUI showing the Source, Target, and Review workspace" src="docs/screenshots/tui-demo.gif"></td>
  </tr>
  <tr>
    <td align="center">Keyboard and mouse</td>
  </tr>
</table>

> [!CAUTION]
> Bootable is unsigned. Check the selected drive, keep backups, and use media you can erase.

Install
-------

Linux or macOS:

```sh
# Desktop app
curl -fsSL https://bootable.palash.dev/install.sh | sh -s -- --gui

# TUI
curl -fsSL https://bootable.palash.dev/install.sh | sh -s -- --tui

# Both
curl -fsSL https://bootable.palash.dev/install.sh | sh -s -- --all
```

Windows: download and extract the ZIP, then run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -Variant All
```

The interface stays unprivileged. Administrator access is requested only for an approved write. See the [download page](https://bootable.palash.dev/download.html) for requirements, checksums, and known limits.

Run
---

```sh
bootable-desktop  # Desktop app
bootable          # TUI
```

Project links
-------------

[Website](https://bootable.palash.dev) ·
[Features](https://bootable.palash.dev/features.html) ·
[Guides](https://bootable.palash.dev/guides.html) ·
[FAQ](https://bootable.palash.dev/faq.html) ·
[Safety](docs/safety.md) ·
[Architecture](docs/architecture.md) ·
[Validation](docs/validation.md) ·
[GUI/TUI parity](docs/ui-parity.md) ·
[Contributing](CONTRIBUTING.md) ·
[Changelog](CHANGELOG.md) ·
[Report a problem](https://github.com/debpalash/bootable/issues/new?template=bug-report.yml)

Build and test
--------------

```sh
cargo build --release --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

See [validation](docs/validation.md) for platform dependencies and destructive test isolation.

License
-------

[Apache-2.0](LICENSE)
