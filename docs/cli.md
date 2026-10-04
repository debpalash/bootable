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
| 4    | verification_failed     | A checksum or post-write byte verification did not match. Treat the media as unusable.   |

Exit 3 covers every safety refusal: a missing `--confirm`, a phrase that does not match the plan,
and a target that is a system disk, an internal disk, or read-only. Exit 4 covers the
publisher-checksum check on downloads and the SHA-256 comparison after a write.

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
  instead of the text plan.
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
```

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

Windows installer options (`--windows-partition-scheme`, `--bypass-windows-11-requirements`,
`--allow-windows-offline-account`, `--windows-local-account`, `--copy-windows-regional-options`,
`--minimize-windows-data-collection`, `--disable-windows-bitlocker`, `--windows-quality-of-life`,
`--use-windows-ca-2023`, `--apply-windows-skusi-policy`, `--force-windows-s-mode`) and
`--bad-block-check off|1|2|4` are shared by `plan`, `write`, and `flash`.

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
