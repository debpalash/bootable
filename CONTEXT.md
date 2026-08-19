# Bootable Media

Bootable creates verified installation and recovery media on removable drives while keeping destructive consequences explicit.

## Language

**Boot image**:
A local ISO, IMG, RAW, or compressed disk image that has been inspected and classified before use.
_Avoid_: Source file, media file

**Target drive**:
The whole removable USB or SD device selected to receive a boot image.
_Avoid_: Disk, destination, output device

**Discovery session**:
The current catalog source, search, selection, and loading outcome used to find a boot image.
_Avoid_: Catalog screen, browser state

**Download job**:
A durable request to fetch, verify, and inspect a catalog image at a chosen local path.
_Avoid_: Transfer, download task

**Write plan**:
The reviewed snapshot of one boot image, one target drive, the write strategy, required tools, ordered operations, and permanent consequences.
_Avoid_: Configuration, write request

**Reviewed write**:
The lifecycle from opening a write plan through consequence acknowledgment, execution, verification, cancellation, or terminal failure.
_Avoid_: Flash action, burn operation

**Privileged write**:
Execution of an acknowledged write plan by the narrowly elevated helper after it independently revalidates the target drive.
_Avoid_: Root mode, elevated app

**Windows installer construction**:
Creation of bootable Windows setup media from an inspected installer image and selected Windows setup options.
_Avoid_: Windows burn, ISO extraction
