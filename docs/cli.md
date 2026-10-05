# Command-line reference

`bootable` with no subcommand opens the terminal UI (it needs a terminal on stdout). Every
subcommand below runs headless and is safe to script. Run `bootable <command> --help` for the
full option list.

## Exit codes

| Code | Name                    | Meaning                                                                                  |
| ---- | ----------------------- | ---------------------------------------------------------------------------------------- |
| 0    | ok                      | The command completed.                                                                   |
| 1    | error                   | Any other failure: I/O, network, missing device, missing privileges, stale plan.         |
| 2    | usage                   | Bad arguments (reported by the parser), out-of-range release index, no terminal for TUI. |
| 3    | confirmation_required   | Nothing was written: `--confirm` missing, wrong phrase, or the target was refused.       |
| 4    | verification_failed     | A checksum, signature, or post-write byte verification failed or was refused (including `--require-signature`). Treat the media as unusable. |

Exit 3 covers every safety refusal: a missing `--confirm`, a phrase that does not match the plan,
and a target that is a system disk, an internal disk, or read-only. Exit 4 covers the
publisher-checksum check on downloads, the SHA-256 comparison after a write, and signature
refusals: a download refused by `--require-signature` (no verified publisher signature), or a
signature that names a pinned key but does not verify (see [signatures.md](signatures.md)).

Errors are printed to stderr as `error: <message>`. For commands that take `--json`, errors are
instead a single JSON object on stderr:

```json
{"error": {"kind": "confirmation_required", "exit_code": 3, "message": "..."}}
```

`kind` is one of `error`, `usage`, `confirmation_required`, `verification_failed`.

## Scripting notes

- Machine-readable output goes to stdout; progress and diagnostics go to stderr. `--json`
  commands print one pretty-printed JSON value and nothing else on stdout.
- `--json-progress` (on `download`, `write`, `flash`) switches stdout to newline-delimited JSON
  events: `{"event":"progress","data":{...}}`, then exactly one terminal event, either
  `{"event":"finished"}` or `{"event":"failed","data":{"message","kind","exit_code"}}`. A
  `write`/`flash` without `--confirm` emits `{"event":"confirmation_required","data":{"confirmation_phrase","plan"}}`
  instead of the text plan; that event is the terminal one and the exit code is 3.
  The guarantee covers early failures too: a missing or ineligible target, a source that cannot be
  planned, a failed fetch, and a refused signature each produce a `failed` event (with the same
  `kind` and `exit_code` as the process) before the command exits. The only failures with no event
  are argument errors rejected by the parser (exit 2), which happen before the command starts and
  are reported on stderr.
- After a catalog image is fetched (`download`, and `flash` with a slug), `--json-progress` also
  emits one `integrity` event before the terminal event. See
  [Integrity reporting](#integrity-reporting).
- Targets are given by device id or path exactly as `bootable devices` prints them. Re-resolve the
  target immediately before a write; ids are the more stable handle across re-plugs.
- Writing needs elevated rights. The CLI asks the platform's authorization helper (pkexec, UAC, or
  the macOS authorization dialog) when it is not already privileged.
- Shell completions are generated from the live command definition, so they always match the
  installed version.

## The confirmation model

Writing erases the target, so every write path goes through one gate:

1. You name the target explicitly. Nothing is ever auto-selected, and `flash` and `write` have no
   "first removable drive" shortcut.
2. The target must pass the eligibility checks in the planner: removable, writable, and not the
   system disk. Ineligible targets are refused with exit 3.
3. A plan is built and its destructive steps are listed. The plan carries an exact confirmation
   phrase (for example `ERASE /dev/sdb ...`).
4. The write only starts if `--confirm` repeats that phrase byte for byte. Without it, the plan
   and the phrase are printed and the command exits 3 without touching the device. A wrong phrase
   also exits 3.

The usual automation pattern is therefore two runs: one dry run to read the phrase from the plan
(`bootable plan --json`, field `confirmation_phrase`, or the exit-3 output of `write`/`flash`),
then a second run with `--confirm`. The phrase is derived from the device, so a script should read
it from the plan for the specific device rather than hard-coding it, and a human or policy layer
should approve it.

## Commands

### catalog

List popular distributions from DistroWatch (an interest ranking, not market share).

```sh
bootable catalog --limit 10
bootable catalog --json | jq -r '.[].slug'
```

### releases

Resolve current ISO downloads for a catalog slug. Indexes shown here are what `--index` takes in
`download` and `flash`.

```sh
bootable releases linuxmint
bootable releases linuxmint --json
```

### download

Download a catalog ISO, verify it against the publisher checksum, and inspect it. Exit 4 if the
checksum does not match (the staged file is discarded).

```sh
bootable download linuxmint --index 0 --output ~/Downloads/mint.iso
bootable download linuxmint --json-progress
bootable download ubuntu --require-signature
```

`--require-signature` refuses the download, before any image byte is transferred, unless the
publisher's checksum manifest carries a verified signature from a key Bootable pins (see
[signatures.md](signatures.md)). A publisher with no signature, or whose signature could not be
used, is refused. The command exits 4 (`verification_failed`) with a message such as
`download refused: a verified publisher signature is required but this image has only: ...`;
with `--json-progress` the same refusal is a `failed` event with `"kind":"verification_failed"`
and `"exit_code":4`. Without the flag a missing signature is not an error: the download proceeds
and the integrity line says so.

#### Integrity reporting

After a successful catalog download the command reports how well the image was authenticated,
using the same wording as the graphical and terminal interfaces:

```
Ready to write: /home/me/Downloads/ubuntu.iso
Kind: ...
Size: ...
Integrity: Signature verified · Ubuntu (key D94A A3F0 EFE2 1092) · SHA-256 matches signed manifest
```

or, when the publisher normally signs but no usable signature was found:

```
Integrity: Publisher checksum verified · signature not verified (signature expected but unavailable: ...)
```

With `--json-progress` the line becomes one event, emitted after the last `progress` event and
before `finished`:

```json
{"event":"integrity","data":{"label":"Signature verified · Ubuntu (key D94A A3F0 EFE2 1092) · ...","signature_verified":true,"signature_expected_but_unverified":false}}
```

`signature_verified` is true only for a signature from a pinned key.
`signature_expected_but_unverified` is true when the publisher is known to sign but only the bare
checksum could be used, a possible downgrade; scripts that must not accept that case should pass
`--require-signature` rather than inspect the event. `pi-download` does not report integrity.

### pi-images and pi-download

List and fetch official Raspberry Pi Imager images; `pi-download` verifies and extracts.

```sh
bootable pi-images --device pi5 --limit 10 --json
bootable pi-download 0 --output raspios.img
```

### devices

List removable media. System and internal disks are not offered as targets.

```sh
bootable devices
bootable devices --json | jq -r '.[] | select(.removable) | .id'
```

### inspect

Identify an image (kind, size, warnings) without reading it onto a device.

```sh
bootable inspect ubuntu.iso --json
```

### checksum

Compute a digest. `--algorithm` defaults to `sha256`.

```sh
bootable checksum ubuntu.iso
bootable checksum ubuntu.iso --algorithm sha256 --json
```

### backup

Copy a device to an image file before you erase it. Refuses to overwrite an existing file.

```sh
bootable backup /dev/sdb sdb-backup.img
```

### plan

Show exactly what a write would do, including the confirmation phrase. Read-only.

```sh
bootable plan ubuntu.iso /dev/sdb
bootable plan ubuntu.iso /dev/sdb --json | jq -r .confirmation_phrase
bootable plan win11.iso /dev/sdb --windows-partition-scheme gpt --bad-block-check off
```

Windows installer options (`--windows-partition-scheme`, `--windows-boot-firmware`,
`--bypass-windows-11-requirements`,
`--allow-windows-offline-account`, `--windows-local-account`, `--copy-windows-regional-options`,
`--minimize-windows-data-collection`, `--disable-windows-bitlocker`, `--windows-quality-of-life`,
`--use-windows-ca-2023`, `--apply-windows-skusi-policy`, `--force-windows-s-mode`) and
`--bad-block-check off|1|2|4` are shared by `plan`, `write`, and `flash`.

`--windows-boot-firmware <uefi|bios-uefi>` (default `uefi`) is **experimental**. `bios-uefi` also
makes Windows installer media boot on legacy BIOS (CSM) machines; it is currently written only by
the Linux adapter and is not yet verified against Microsoft's real `bootmgr` or on real hardware
(see [legacy-bios.md](legacy-bios.md)). It needs `--windows-partition-scheme mbr` and media of at most 2 TiB;
`plan`, `write`, and `flash` print core's refusal verbatim and write nothing
when the combination is not allowed.

```sh
bootable plan win11.iso /dev/sdb --windows-partition-scheme mbr --windows-boot-firmware bios-uefi
```

### write

Write and verify a local image. Without `--confirm` it prints the plan and exits 3.

```sh
bootable write ubuntu.iso /dev/sdb                      # prints plan, exit 3
bootable write ubuntu.iso /dev/sdb --confirm 'ERASE ...'  # writes and verifies
```

### flash

One step from source to bootable media. `SLUG_OR_IMAGE` is a local file or a catalog slug.
`TARGET` is required and is never inferred.

- A value that is an existing file, or looks like a path (contains `/`, `\`, or `.`, or starts
  with `~`), is treated as a local image. A bare word such as `linuxmint` is a catalog slug.
- For a slug, the target is checked for existence and eligibility before any download starts.
  Then the release (`--index`, default 0) is downloaded to `--output` (default: the release file
  name in the current directory) and verified exactly as `download` does.
- `--require-signature` applies to that download exactly as it does for `download` (exit 4 when
  the image has no verified publisher signature, before any image byte is fetched), and the
  integrity line or `integrity` event is printed once the download completes. The flag is a
  usage error (exit 2) with a local image, which has no publisher signature to check.
- The image is then planned and written through the same confirmation gate as `write`, so it
  exits 3 without `--confirm` and the downloaded file is kept. Re-run with the printed path to
  skip the download.

```sh
# Local image
bootable flash ubuntu.iso /dev/sdb
bootable flash ubuntu.iso /dev/sdb --confirm 'ERASE ...'

# Catalog slug
bootable flash linuxmint /dev/sdb --index 0 --output mint.iso
bootable flash mint.iso /dev/sdb --confirm 'ERASE ...'

# Unattended with structured progress
bootable flash mint.iso /dev/sdb --confirm "$PHRASE" --json-progress
```

On success the command prints `Done: <image> written and verified on <target>` (or the
`finished` event with `--json-progress`). A failed byte comparison exits 4.

### completions

Print a completion script for `bash`, `zsh`, `fish`, `powershell`, or `elvish`.

```sh
bootable completions bash > ~/.local/share/bash-completion/completions/bootable
bootable completions zsh > "${fpath[1]}/_bootable"
bootable completions fish > ~/.config/fish/completions/bootable.fish
bootable completions powershell >> $PROFILE
bootable completions elvish > ~/.config/elvish/lib/bootable.elv
```
