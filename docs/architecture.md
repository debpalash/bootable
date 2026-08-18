# Architecture

The UI applications know about images, devices, plans, and progress. They do not know how a GPT is
created, how WIM parts are named, or how a raw target is verified.

```text
GPUI desktop ─┐
              ├─ Bootable API ─ inspection + policy + plan ─ native platform adapter
Ratatui TUI ──┘                                          ├─ device discovery
                                                        ├─ privileged operations
                                                        └─ verification
```

`bootable-core` is the deep module. Its public surface is intentionally small:

- `discover_devices`
- `inspect_image`
- `plan` / `prepare`
- `write`

The platform adapter owns device enumeration and destructive mechanics. On Linux the unprivileged
GUI/TUI sends the reviewed serializable plan to a root-owned `bootable-helper` launched by
`pkexec`. The helper exposes only the write/cancel protocol, emits JSON progress over stdout, and repeats
device discovery, stable-identity checks, removable/system-disk policy, capacity checks, unmounting,
writing, and verification. It never trusts a stale device path from the UI.

The native adapters use the same seam and repeat the same plan-bound checks:

- Windows: USB-only PowerShell `Get-Disk` inventory, stable-ID refresh, volume detachment, and
  `PhysicalDrive` raw I/O through a fixed helper under protected Program Files. The UI creates a
  256-bit one-use token, accepts only an IPv4 loopback connection carrying that token, and invokes
  the helper with `Start-Process -Verb RunAs`. The helper repeats the reviewed-plan checks and
  returns progress/cancellation events without elevating the frontend.
- macOS: I/O Registry whole-removable inventory, root-disk exclusion, stable-ID refresh,
  `diskutil` unmounting, and `/dev/rdisk` raw I/O through a fixed root-owned helper. The UI opens a
  private Unix socket, macOS presents its administrator prompt, and the helper carries the same
  reviewed-plan/cancel/progress protocol without elevating the frontend.

Windows also mounts installer ISOs with the native storage cmdlets, preflights the complete source
before clearing the target, creates a bounded FAT32 partition, splits oversized WIMs with DISM,
applies the shared answer-file model, and verifies the copied boot tree. Platform-native macOS
Windows FAT32 creation remains open work. All platforms launch narrow privileged helpers while the
full UI remains unprivileged.

Image classification and policy remain shared across all three operating systems.

Compressed raw images and hybrid ISOs remain a raw-write strategy. Inspection measures the expanded
stream off the UI thread; execution selects the decoder from the serialized image kind, hashes bytes
while writing, and verifies that exact expanded hash from the target. Neither frontend handles archive
formats or estimates target capacity independently.
