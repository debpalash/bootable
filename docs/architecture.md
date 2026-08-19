# Architecture

Bootable has two presentation adapters over shared product sessions. GPUI and Ratatui translate
input and render state; they do not schedule catalog requests, classify worker outcomes, authorize a
write, interpret the privileged protocol, or define Windows-media validity.

```text
GPUI desktop ─┐
              ├─ DiscoverySession ─ catalog cache + DistroWatch/Raspberry Pi adapters
Ratatui TUI ──┤
              ├─ ManagedDownloadSession ─ durable DownloadLedger + verified transfer
              ├─ ReviewedWriteSession ─ plan + acknowledgment + progress + terminal outcome
              └─ Bootable ─ inspection + policy + plan ─ native platform adapter
                                                   ├─ device discovery
                                                   ├─ privileged launch adapter
                                                   └─ write + verification

native launch adapters ─ Privileged protocol ─ bootable-helper
native media adapters  ─ Windows media rules ─ mounted installer tree
```

## Product sessions

- `DiscoverySession` owns source/preset selection, the six catalog loading states, duplicate-load
  suppression, and the expected distribution slug. A response for an older selection cannot alter
  the active profile.
- `ManagedDownloadSession` owns the one-active-worker rule, retry queuing, FIFO continuation,
  progress, pause/cancel control, and typed completion. `DownloadLedger` remains the durable seam for
  crash recovery and partial-download ownership.
- `ReviewedWriteSession` owns the immutable reviewed plan, consequence modal, acknowledgment,
  active-operation lock, progress, cancellation, and typed terminal result. A frontend cannot obtain
  a write launch before review and acknowledgment.

These sessions expose state for rendering and worker launches for adapter-specific execution. The
frontends do not own the transition invariants.

## Privileged writes

The unprivileged GUI/TUI sends the reviewed serializable plan to a narrowly elevated
`bootable-helper`. The shared privileged-protocol module serializes the request, transports
cancellation, accepts progress, requires exactly one terminal event, and resolves helper/protocol/
process failures in one order. OS adapters only establish the secure transport and authorization:

- Linux launches the fixed helper through `pkexec`.
- Windows uses a 256-bit one-use loopback handshake and a protected Program Files helper launched
  with `Start-Process -Verb RunAs`.
- macOS uses a private Unix socket and a fixed root-owned helper launched through the system
  authorization prompt.

The helper repeats device discovery, stable-identity checks, removable/system-disk policy, capacity
checks, unmounting, writing, and verification. It never trusts a stale device path from the UI.

## Media construction

The shared Windows-media module owns answer-file application, case-insensitive payload lookup,
FAT32 size policy, and post-copy boot-tree verification. Native adapters retain mounting,
partitioning, file copying, WIM splitting, CA-2023 tool invocation, and teardown because those are OS
mechanics.

Compressed raw images and hybrid ISOs remain a raw-write strategy. Inspection measures the expanded
stream off the UI thread; execution selects the decoder from the serialized image kind, hashes bytes
while writing, and verifies that exact expanded hash from the target.
