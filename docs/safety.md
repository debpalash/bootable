# Safety model

Writing boot media is intentionally destructive. Bootable applies all of these gates before the
target is opened for writing:

1. The target must be a whole disk discovered by the native platform adapter.
2. It must be USB-attached or explicitly reported removable.
3. It must not be read-only.
4. It must not contain the running system root.
5. The image plus strategy overhead must fit.
6. Interactive users must review a blocking consequence modal, verify the physical target, and
   explicitly acknowledge permanent erasure. The CLI requires the exact phrase generated from the
   device path and serial suffix. Both paths pass the plan-bound phrase into the core writer.
7. Immediately before writing, the engine re-discovers the target by stable ID and compares its
   capacity and safety flags with the reviewed plan.
8. Mounted child filesystems are unmounted before any destructive operation.

Raw writes are SHA-256 hashed while streaming, flushed, and then read back for comparison over
exactly the image length. Compressed XZ, gzip, Zstandard, and bzip2 sources are fully measured before
planning so capacity checks use expanded bytes; they are decompressed directly into the writer and
the expanded byte stream is what the target must match. Trailing bytes on a larger USB device are
irrelevant to that comparison.

Windows media is created as one FAT32 Microsoft Basic Data partition. Installer payloads larger
than FAT32 permits are split into `install.swm`, `install2.swm`, and subsequent parts with wimlib.
Microsoft UDF-only installer trees are enumerated through 7-Zip when their ISO-9660 bridge does not
expose the real files. Verification checks the boot files, validates the complete split-WIM set,
and rejects any remaining file over the FAT32 limit.

## Privilege boundary

The Linux CLI write command may run directly as root. The interactive TUI and desktop remain
unprivileged: after consequence acknowledgment they launch the root-owned narrow `bootable-helper`
through `pkexec`, which triggers the system administrator-authentication prompt. Bootable rejects
user-writable helper binaries. The reviewed plan and cancellation commands use a pipe rather than a
temporary file, and helper progress returns as JSON events. The helper repeats target identity and
safety checks immediately before erasure. Device-list mutation and window/back/quit actions are locked
during an active write. A dedicated safe stop action cancels on a chunk boundary, terminates controlled
media tools, and flushes completed raw writes; the result marks the media incomplete and requiring a
full rewrite.
