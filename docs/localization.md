# Localization

Bootable's desktop (GUI) and terminal (TUI) interfaces share one translation
catalog that lives in `bootable-core`. This is what keeps the parity invariant
true across languages: both interfaces ask core for the same `Message`, in the
same `Locale`, and get the same words.

`en` is the source of truth. A locale, or a single message inside a locale,
that has no translation falls back to English. Nothing ever renders an empty
or missing string.

## What exists today

| Piece | Where |
| --- | --- |
| `Locale` (identity, parsing, detection, plural rules, direction) | `crates/bootable-core/src/locale.rs` |
| `Message` (typed keys), lookup, interpolation, plurals, coverage | `crates/bootable-core/src/messages.rs` |
| Per-locale tables (embedded with `include_str!`) | `crates/bootable-core/locales/<tag>.lang` |
| Persisted override | `Preferences.language: Option<Locale>` |
| Converted core strings | help guide, drive detail rows, target eligibility, removable-media status, readiness/guidance/status text, step titles, progress phase names, integrity labels |

There are no new dependencies. The catalog format is a small `key = value`
file rather than Fluent. Fluent would be the right choice if messages needed
grammatical selectors beyond plurals (gender, case); Bootable's messages are
short status lines, plural handling is the only agreement needed, and every
transitive dependency of a tool that writes to disks is a liability. If that
changes, `Message::text/format/plural` is the only surface adapters touch, so
the backend can be swapped without touching them.

## Public API (stable surface)

All of these are re-exported from `bootable_core`.

```text
Locale                         enum: En, Es, Fr, De, PtBr, ZhHans, Ja, Ru, Hi
  Locale::ALL                  every recognized locale, source first
  Locale::SOURCE               Locale::En
  Locale::available()          English + locales with >= 1 translation (for pickers)
  Locale::parse(&str)          "pt_BR.UTF-8", "zh-Hans-CN", "fr" -> Option<Locale>
  "...".parse::<Locale>()      FromStr, Err = UnsupportedLocale
  Locale::negotiate(list)      first supported entry of an ordered preference list
  Locale::detect_system()      LANGUAGE/LC_ALL/LC_MESSAGES/LANG (+ macOS AppleLanguages)
  Locale::detect_with(getter)  same, with an injectable environment (tests)
  Locale::resolve(explicit)    explicit override, else system, else English
  locale.tag()                 "pt-BR" (also the Display form and the serde form)
  locale.native_name()         "Português (Brasil)"; for the language picker
  locale.english_name()
  locale.direction()           TextDirection::{LeftToRight, RightToLeft}
  locale.uses_wide_characters()
  locale.plural_category(n)    PluralCategory::{One, Few, Many, Other}
  locale.coverage()            Coverage { translated, total, missing }

Message                        typed key enum; Message::ALL, message.key()
  message.text(locale)         -> &'static str   (no placeholders; plural -> "other")
  message.format(locale, &[("name", &value), ...]) -> String
  message.plural(locale, count, &[...])            -> String   ({count} is implicit)
  message.is_plural()

Preferences
  preferences.language         Option<Locale>   (None = follow the system)
  preferences.locale()         effective Locale (override > system > English)

Localized variants (each English-only original still exists and is unchanged)
  help_intro(locale) -> &'static str                  HELP_INTRO
  help_sections(locale) -> Vec<LocalizedHelpSection>  HELP_SECTIONS
  device_details_in(locale, &Device) -> Vec<DetailRow>            device_details
  target_eligibility_label_in(locale, &Device) -> &'static str    target_eligibility_label
  removable_media_status_in(locale, &[Device]) -> String          removable_media_status
  ReviewReadiness::{action_label_in, guidance_in}(locale)         action_label, guidance
  WorkspaceProgress::status_in(locale)                            status
  WorkspaceProgress::step_titles(locale) -> [&str; 3]             "Source/Target/Review & write"
  ProgressPhase::label_in(locale)                                 Display (English)
  IntegrityState::{label_in, completion_message_in,
                   finalized_message_in, ready_message_in}(locale, ...)
```

The English-only functions and constants delegate to the catalog in `en`, so
English output is byte-for-byte what it was. `HELP_SECTIONS`/`HELP_INTRO` are
the one exception: they remain hand-written `const`s (a `const` cannot read the
catalog), and a test pins them to the English catalog so they cannot drift.
Adapters should move to `help_sections(locale)`.

### Wiring an adapter

```rust
// once at startup, and again whenever the user changes the setting
let locale = preferences.locale();

// plain labels
let status = removable_media_status_in(locale, &devices);
let rows = device_details_in(locale, &device);

// the picker: (tag stored in preferences, label shown to the user)
for choice in Locale::available() {
    show(choice.tag(), choice.native_name());
}
// picking "System default" sets preferences.language = None
```

Store the `Locale` in app state and thread it into every call; do not call
`detect_system()` per frame (it reads the environment, and on macOS can run
`defaults`). Changing the language must re-render everything, including text
that was cached as a `String` (for example `self.status`). Prefer storing a
`Message` plus its arguments and rendering at draw time, or clearing the cached
status when the language changes.

## Locales and translation quality

| Tag | Language | State |
| --- | --- | --- |
| `en` | English | Source of truth, complete (78/78) |
| `es` | Spanish | Machine-quality draft, complete. Needs native review. |
| `fr` | French | Machine-quality draft, complete. Needs native review (also NBSP typography, see the file header). |
| `de` | German | Machine-quality draft, complete. Needs native review. |
| `pt-BR` | Portuguese (Brazil) | Machine-quality draft, complete. Needs native review. |
| `ru` | Russian | Machine-quality draft, complete, with `one/few/many/other` plurals. Needs native review. |
| `ja` | Japanese | Machine-quality draft, complete. Needs native review. |
| `zh-Hans` | Simplified Chinese | Machine-quality draft, complete. Needs native review. |
| `hi` | Hindi | Recognized, **no translations**; falls back to English everywhere. Not offered by `Locale::available()`. |

Every shipped table is a first draft by a model, not a professional or native
translation. They are a starting point for reviewers, not a release-quality
claim. In particular, review:

* the erase/safety wording in `help.intro`, `help.target.choose.detail`, and the
  `workspace.*` / `readiness.*` strings, which tell people nothing has been
  written yet; a mistranslation here is a safety problem, not a style problem;
* the integrity strings (`integrity.*`): "checksum" versus "hash", and keeping
  "signature verified" strictly stronger than "checksum verified";
* terminology choices: image (imagen / image / Abbild / imagem / образ /
  イメージ / 镜像), drive, removable, "writing" a disk (gravar vs. escribir).

Partial translations are supported: omit any key and English is used for it.
Hindi is the worked example (an empty table).

Whenever a reviewer confirms a table, remove its `STATUS: machine-quality`
header line and update this table.

## Catalog format

```text
# comment
key = value with {placeholder}
literal.braces = write {{ and }} for literal braces
multi.line = first\nsecond
count.thing.one = {count} file
count.thing.other = {count} files
```

* One message per line; leading/trailing whitespace is trimmed.
* Keys: `a-z`, `0-9`, `.`, `_`.
* Escapes: `\n`, `\\`. Anything else after a backslash is an error.
* Placeholders: `{name}` where `name` is `a-z`/`_`.
* Plurals: `key.one`, `key.few`, `key.many`, `key.other`. The set required for
  a locale is exactly its CLDR integer categories plus `other`
  (`Locale::plural_categories()`): `one`+`other` for en/es/fr/de/pt-BR/hi,
  `one`+`few`+`many`+`other` for ru, `other` only for ja/zh-Hans. `other` is
  also the fallback when a count's category is missing. Spanish, French and
  Portuguese additionally have a CLDR `many` category for exact multiples of
  one million, which is not modeled.

### Adding or changing a message

1. Add the variant and key to the `messages!` list in `messages.rs`.
2. Add the key to `locales/en.lang` (this is the only place English lives).
3. Use it: `Message::Foo.text(locale)`, `.format(...)`, or `.plural(...)`.
4. Other locales may lag; missing keys fall back to English and are reported
   by `cargo test -p bootable-core messages -- --nocapture` (look for
   `i18n coverage`).

### Adding a locale

1. Add the variant to `Locale` (`ALL`, `tag`, `native_name`, `english_name`,
   `parse`, `plural_categories`, `plural_category`, and `uses_wide_characters`
   / `direction` if needed) and a `source()` arm in `messages.rs`.
2. Create `locales/<tag>.lang`.
3. `cargo test -p bootable-core`.

### Guarantees enforced by tests

* Every catalog parses without errors (bad line, duplicate key, bad escape,
  malformed placeholder).
* `Message` and `en.lang` agree exactly, in both directions.
* A locale may not define a key `en` lacks.
* A translation's placeholders must equal English's (`{count}` is implicit for
  plurals).
* Plural messages define exactly their locale's categories.
* Missing translations never fail the build; they are printed as
  `i18n coverage <tag>: translated/total`.
* The English help text equals the legacy `HELP_SECTIONS` constants; help has
  identical section/entry shape in every locale; terminal key chords are never
  translated.

## Detection and the override

Order of precedence: `Preferences.language` (explicit) > system > English.

System detection (`Locale::detect_system`):

1. The effective locale is the first non-empty of `LC_ALL`, `LC_MESSAGES`,
   `LANG`. If it is `C`, `C.*` or `POSIX`, no preference is expressed
   (English).
2. Otherwise the colon-separated `LANGUAGE` list is tried first (so
   `LANGUAGE=ko:ru` yields Russian), then the effective locale itself.
3. macOS, when none of those variables are set (apps launched from Finder): the
   `AppleLanguages` global default is read with `defaults read -g
   AppleLanguages` (fixed argv, no shell, no `unsafe`).
4. Windows: only the POSIX-style variables are read. The Windows UI language is
   not available without FFI (`GetUserDefaultLocaleName`), which this crate
   forbids. Windows users therefore get English until they choose a language
   (`Preferences.language`); a future FFI-capable adapter crate can feed
   `Locale::negotiate` with the OS list without core changes.

Mapping is deliberately conservative: `pt` and `pt-PT` map to `pt-BR` (the only
Portuguese shipped), `zh`/`zh-CN`/`zh-Hans` map to `zh-Hans`, while
`zh-TW`/`zh-HK`/`zh-Hant` map to nothing (Traditional text is not a safe
substitute for Simplified, so those users get English).

`Preferences.language` is serialized as its tag (`"pt-BR"`), is
`#[serde(default)]`, and an unknown tag deserializes to `None` rather than
discarding the file. `PREFERENCES_VERSION` stays `1`; files written before the
field existed load normally, and an older build ignores the new field.

## Things that must never be localized

* **The erase confirmation phrase** (`plan.confirmation_phrase`, e.g.
  `ERASE /dev/sdb TEST`). It is typed verbatim and compared exactly; it is a
  protocol token, not prose. Surround it with translated text, never translate
  it.
* Device paths, device ids, serials, file names, checksum algorithm names
  (`SHA-256`), key fingerprints, URLs, command-line flags and the privileged
  helper protocol.
* Key chords (`Ctrl+O`, `Esc`, `↑ ↓ · j k`). Only descriptions of inputs such
  as "Stop button" are translated.
* Text that originates outside Bootable (DistroWatch descriptions, distribution
  names, Raspberry Pi catalog text, `signature_note` reasons). These display as
  received.

Numbers: `format_bytes` is locale-neutral (`16.0 GiB`, binary units). Digit
grouping and decimal separators are not localized yet; keep byte sizes through
`format_bytes` rather than formatting in adapters so there is one place to add
that.

## Right-to-left and wide characters

No right-to-left locale ships, but nothing in the API assumes left-to-right:
`Locale::direction()` exists, always returns `LeftToRight` today, and adapters
should consult it rather than hard-coding.

When the first RTL locale (Arabic, Hebrew, Persian) is added:

* Set `direction()` to `RightToLeft` for it and add its plural categories
  (Arabic needs `zero/one/two/few/many/other`; `PluralCategory` currently models
  only the integer categories `one/few/many/other` and must grow `zero` and
  `two`).
* **GUI (GPUI):** mirror layout (sidebar and step order, chevrons such as
  `Select →` and `Open →`, the `Source → Target → Review & write` flow) and
  verify the toolkit's shaping and bidi support before shipping; text widgets
  must set paragraph direction from `direction()`.
* **TUI:** terminals generally do not implement the Unicode bidi algorithm or
  Arabic shaping, so RTL text may render visually reversed or unjoined. Treat a
  TUI RTL locale as unsupported unless the terminal is known to do bidi, and
  say so in the picker.
* Embedded LTR runs (paths, `/dev/sdb`, `SHA-256`, fingerprints, the erase
  phrase) inside RTL sentences need isolation (U+2066 LRI ... U+2069 PDI) so
  they do not reorder. Core will need an `isolate()` helper applied to
  `{placeholder}` data for RTL locales.
* Arrows in messages (`→`, `Select →`) are directional and need a mirrored
  variant per direction. Keep them out of translatable text where possible and
  let the adapter draw them.

Wide characters (zh-Hans, ja; `uses_wide_characters()`):

* East Asian glyphs occupy two terminal columns and often one `char`. The TUI
  must measure with display width (the `unicode-width` crate, already a
  transitive dependency of ratatui), not `chars().count()`, for truncation,
  padding, column alignment, and `Constraint::Length` budgets. The existing
  `…` truncations and fixed-width columns in the TUI assume one column per
  char and will misalign or overflow.
* Long English labels are often shorter than French/German/Russian text and
  longer than CJK text: budget 30-40% extra width for de/fr/ru/pt-BR/es, and
  do not bake English widths into layouts (several TUI tables and GUI buttons
  size to content today; the compact modes in the TUI and `compact` flag in the
  GUI are the places to check).
* CJK needs a font with coverage in the GUI and a terminal that renders it;
  there is no word spacing, so wrapping must be allowed between any two CJK
  characters (do not wrap only on spaces).
* The French typographic convention uses a (narrow) no-break space before
  `: ; ! ?`; the shipped table uses plain spaces so wrapping stays predictable.

## UI strings still to extract (adapter work)

Everything below is hard-coded in `apps/bootable-tui/src/main.rs` (TUI) or
`apps/bootable-desktop/src/main.rs` (GUI) and not yet in the catalog. Proposed
key names use the prefix `app.` so they do not collide with the core-owned
messages. "Both" means the same English text exists in both apps (move it to one
key). "GUI/TUI differ" marks wording drift that must be unified when extracting,
which the parity invariant requires anyway.

Where several strings share a stem the placeholders are shown as `{name}`.
Text produced by `format!` should become `Message::format`; counts such as
`{n} source error(s)` and `Detected {added} new drive(s)` should become plural
messages instead of the `(s)` pattern, which does not translate.

### Header, brand, global actions

| Key | English | Notes |
| --- | --- | --- |
| `app.name` | Bootable | window title; brand, likely not translated |
| `app.tagline.idle` | Boot media, written deliberately. | GUI |
| `app.header.title.create` | Create boot media | both |
| `app.header.title.review` | Review write plan | GUI header; TUI modal title |
| `app.header.subtitle.create` | One deliberate path from image to removable drive. | TUI; GUI uses `app.header.subtitle.flow` |
| `app.header.subtitle.flow` | Image → removable drive → verified result | GUI |
| `app.header.subtitle.review` | Inspect every operation before confirmation. | GUI |
| `app.action.downloads` | Downloads · {count} | GUI; "Jobs · {count}" compact |
| `app.action.downloads_compact` | Jobs · {count} | GUI |
| `app.action.discover` | Discover images | both |
| `app.action.catalog_open` / `app.action.catalog_close` | Catalog × / Close catalog | GUI |
| `app.action.options` / `app.action.setup_options` / `app.action.hide_options` | Options / Setup options / Hide options | GUI |
| `app.tooltip.guide` | Guide and shortcuts (F1) | GUI |
| `app.tooltip.refresh_drives` | Refresh removable drives | GUI |
| `app.tooltip.refresh_distrowatch` | Refresh DistroWatch data | GUI |
| `app.tooltip.refresh_pi` | Refresh Raspberry Pi catalog | GUI |
| `app.guide.title` | Guide | GUI |
| `app.guide.close` | Close | GUI |
| `app.guide.dismiss_hint` | Press Esc, ? or click to close | TUI |
| `app.footer.keys` | Tab / Shift+Tab focus · Enter select · ? help · q quit | TUI |

### Workspace focus (TUI status when focus moves)

| Key | English |
| --- | --- |
| `app.focus.source` | Source · choose or change the image |
| `app.focus.target` | Target · choose an eligible removable drive |
| `app.focus.setup` | Setup options · configure image-specific choices |
| `app.focus.review` | Review & write · inspect the plan before erasure |
| `app.focus.discover` | Discover images · browse trusted catalogs |
| `app.focus.refresh` | Refresh drives · rescan removable media |

### Source panel

| Key | English |
| --- | --- |
| `app.source.title` | Choose an image |
| `app.source.hint` | The image is inspected before any write is allowed |
| `app.source.formats` | ISO, IMG, RAW, or compressed disk image |
| `app.source.formats_inspected` | ISO, IMG, RAW, or compressed disk image\nInspected before writing |
| `app.source.action.browse` / `change` / `inspecting` | Browse / Change / Inspecting… |
| `app.source.recent.title` | RECENT IMAGES (TUI appends ` · press 1-4`) |
| `app.source.recent.empty` | Images you use appear here for one-click reuse |
| `app.source.recent.in_use` | In use |
| `app.source.dialog.title` | Boot images |
| `app.source.dialog.filter_iso` | ISO images |
| `app.source.dialog.filter_backup` | Raw drive image |
| `app.discover.card.title` | Discover images |
| `app.discover.card.subtitle` | Browse trusted catalogs · Open → |

### Target panel

| Key | English |
| --- | --- |
| `app.target.title` | Choose a drive |
| `app.target.empty` | Connect a removable USB or SD drive, then refresh |
| `app.target.state.blocked` / `selected` / `select` | Blocked / Selected / Select → |
| `app.target.selected_header` | SELECTED DRIVE |
| `app.target.confirm_physical` | Confirm the physical drive before continuing · erasure starts only after review |
| `app.target.confirm_physical_short` | Confirm the physical drive · erasure starts only after review (TUI) |
| `app.target.flag.read_only` / `app.target.flag.system_blocked` | READ-ONLY / SYSTEM—BLOCKED |
| `app.target.erases_data` | ERASES DATA |

Device-name fallbacks, vendor/model strings and `Drive details changed` messages
are in "Status line messages" below.

### Setup options (Windows and Linux/Unix media)

| Key | English |
| --- | --- |
| `app.options.windows.title` | Windows installer options |
| `app.options.windows.title_counted` | Windows installer options  ·  checkboxes  ·  {selected} selected (TUI) |
| `app.options.windows.partition_scheme` | Partition scheme · target firmware |
| `app.options.windows.scheme_status` | Windows partition scheme: {scheme} · target firmware: UEFI |
| `app.options.windows.bypass_hardware` | Bypass TPM, Secure Boot and RAM checks |
| `app.options.windows.local_account` | Expose local/offline account setup |
| `app.options.windows.named_account` | Create local account: {name} |
| `app.options.windows.host_region` | Copy this computer's locale and time zone |
| `app.options.windows.privacy` | Apply privacy-focused OOBE defaults |
| `app.options.windows.bitlocker` | Disable automatic BitLocker encryption |
| `app.options.windows.qol` | QoL: reduce Copilot, OneDrive, Teams, suggestions, and Fast Startup |
| `app.options.windows.ca2023` | Use Windows UEFI CA 2023 signed bootloaders |
| `app.options.windows.skusipolicy` | Apply SkuSiPolicy.p7b Secure Boot revocations |
| `app.options.windows.smode` | Force Windows S Mode (expert) |
| `app.options.windows.installer_ready` | Windows ISO ready · setup choices unlocked |
| `app.options.windows.installer_locked` | Choose a Windows ISO to unlock setup choices |
| `app.options.windows.choose_iso` / `replace_iso` | Choose Windows ISO / Replace Windows ISO |
| `app.options.windows.headline` | Windows installer media · Rufus-inspired workflow (TUI) |
| `app.options.windows.features_available` | Rufus 4.15 inventory below: ✓ available now · ○ not implemented (TUI) |
| `app.options.windows.unavailable_note` | Unavailable items are not clickable. Existing autounattend.xml files are never overwritten. |
| `app.options.linux.title` | Linux / Unix boot media (TUI: `Linux / Unix boot media  ·  active features`) |
| `app.options.linux.layout` | Preserve the complete bootable disk layout |
| `app.options.linux.verify` | Verify the written bytes with SHA-256 |
| `app.options.always_on.layout` / `boot_records` / `byte_verification` / `safe_unmount` | Full disk layout / Boot records / Byte verification / Safe unmount (TUI) |
| `app.options.tools.title` | Media tools |
| `app.options.tools.subtitle` | Verification and backup utilities |
| `app.options.tools.bad_blocks` | Bad blocks · {mode} |
| `app.options.tools.bad_blocks_off` / `bad_blocks_n` | Bad blocks off / Bad blocks {passes}x (TUI colon form: `Bad blocks: off`, `Bad blocks: {passes}x`; GUI/TUI differ) |
| `app.options.tools.verify_image` / `image_folder` / `backup_drive` | Verify image / Image folder / Back up drive |
| `app.options.verification_on` | Verification on · {bad_blocks} |
| `app.windows.feature.*` | Feature inventory (GUI list): Standard Windows installation; GPT or MBR + UEFI FAT32 media; Split WIM files above 4 GiB; Remove TPM / Secure Boot / RAM requirements; Remove online Microsoft-account requirement; Disable data collection / skip privacy questions; Disable automatic BitLocker device encryption; Create a named local administrator account; Copy host locale and time zone; QoL policies for bundled Windows experiences; Windows CA 2023 signed bootloaders; Apply SkuSiPolicy.p7b revocations; Force Windows S Mode; MD5 / SHA-1 / SHA-256 / SHA-512 checksums; Reviewed erase phrase and removable-drive safety; Windows To Go and internal-disk isolation; Legacy BIOS boot and NTFS / UEFI:NTFS media; Fully unattended silent disk installation |
| `app.windows.heading` / `app.windows.subheading` | Windows installer media / Complete Rufus 4.15 Windows inventory · working controls are clearly separated (GUI) |
| `app.windows.coming` | Rufus Windows features still being implemented (GUI) |
| `app.windows.reveal_hint` | Choose an inspected Windows installer ISO to reveal the independent Windows setup checkboxes. No Windows option is applied silently. (GUI) |
| `app.windows.unavailable_warning` | Unavailable items are intentionally not clickable. Silent installation can erase the first disk Windows Setup detects and requires a separate high-friction safety design. (GUI) |

The TUI short labels used as checkbox captions: `Hardware bypass`, `Offline
account`, `Privacy defaults`, `Disable BitLocker`, `Named account`, `Host
region`, `QoL policies`, `CA 2023`, `SkuSiPolicy`, `Force S Mode`, `Local
account`, `Scheme: {scheme}`; these should each be a key (`app.options.short.*`)
and ideally collapse into the long GUI labels where both fit.

### Review and confirmation

| Key | English |
| --- | --- |
| `app.review.title` | Review write plan |
| `app.review.subtitle.idle` | Nothing is written until the consequences are reviewed and acknowledged. (TUI) / Nothing is written until a separate destructive confirmation succeeds. (GUI) (GUI/TUI differ) |
| `app.review.subtitle.writing` | Writing and verification are active • do not unplug the target. (TUI) |
| `app.review.field.source` / `target` / `method` / `consequence` | SOURCE / TARGET / METHOD / CONSEQUENCE |
| `app.review.consequence` | All existing data and partitions on the selected target will be erased. |
| `app.review.ordered_operations` | Ordered operations |
| `app.review.state.writing` | Writing and verification are active |
| `app.review.state.final_confirm` | One final confirmation is required |
| `app.review.warning.writing` | Do not close the app, power off, or unplug the target drive. |
| `app.review.warning.idle` | Review the exact target changes and irreversible consequences before writing. |
| `app.review.hint.open_confirmation` | Open the confirmation to review changes, consequences, and the physical target. (TUI) |
| `app.review.action.stop` / `written` / `retry` / `consequences` | Stop safely / Written & verified / Review & retry / Review consequences |
| `app.review.action.back` | Back |
| `app.review.status.writing` | Writing and verification are active · do not unplug the target |
| `app.review.status.complete` | Complete · the written media passed byte verification |
| `app.review.status.review` | Review the physical target and permanent changes before writing |
| `app.confirm.title` | Confirm permanent changes |
| `app.confirm.subtitle` | Review what Bootable will change and what can go wrong. |
| `app.confirm.badge` | PERMANENT |
| `app.confirm.physical_target` | PHYSICAL TARGET |
| `app.confirm.changes` | Changes to this drive |
| `app.confirm.consequences` | Consequences |
| `app.confirm.consequence.1` | Every existing file and partition on this physical drive will be permanently erased. |
| `app.confirm.consequence.2` | Choosing the wrong drive destroys the data on that drive; confirm its model, path, and capacity below. |
| `app.confirm.consequence.3` | Power loss, closing the app, or unplugging during writing can leave incomplete and unbootable media. |
| `app.confirm.consequence.4` | Bootable rechecks the target identity immediately before erasure and verifies the result afterward. |
| `app.confirm.ack` | I checked the physical target and understand that all of its existing data will be permanently erased. |
| `app.confirm.cancel` / `app.confirm.submit` | Cancel / Confirm erase & write |
| `app.confirm.ack_required` | Acknowledge the consequences before confirming the write |
| `app.result.success.title` / `.body` | Write complete / The image was written and verified. The removable drive can now be safely removed. |
| `app.result.cancelled.title` / `.body` | Write cancelled before erasure / Administrator authentication was cancelled or denied. |
| `app.result.stopped.title` / `.body` | Write stopped safely / The media is incomplete and must be rewritten before use. |
| `app.result.failed.title` | Write failed |

The confirmation phrase prompt must show the untranslated phrase (see "Things
that must never be localized").

### Discovery (distributions, ISOs, Raspberry Pi)

| Key | English |
| --- | --- |
| `app.discover.title` | Discover distributions |
| `app.discover.disclaimer` | DistroWatch page-hit ranking measures interest—not quality or market share (GUI) / DistroWatch popularity · six-month page-hit ranking + Interest indicator only; not usage, quality, or market share. (CLI) (differ) |
| `app.discover.search_placeholder` | Search by name, slug, or base family… (TUI adds `  / to type`) |
| `app.discover.section.popular` / `search` / `arch` / `debian` / `omarchy` | POPULAR · SIX MONTHS / SEARCH RESULTS / ARCH-BASED / DEBIAN-BASED / OMARCHY |
| `app.discover.quick.arch` / `debian` / `omarchy` / `windows` / `raspberry_pi` | Arch / Debian / Omarchy / Windows / Raspberry Pi |
| `app.discover.state.retry` / `refresh` | Retry / Refresh |
| `app.discover.item.select` / `selected` | Select → / Selected |
| `app.discover.item.independent` / `directory` | Independent / Directory; DistroWatch directory |
| `app.discover.item.size_unknown` | Size unknown |
| `app.discover.item.publisher_checksum` / `no_publisher_checksum` | Publisher {algorithm} / No publisher checksum |
| `app.discover.item.https_only` | HTTPS only |
| `app.discover.detail.empty` | Choose a distribution to load its profile and ISO files (TUI) / Choose a distribution to resolve its current ISO files (GUI) (differ) |
| `app.discover.detail.os_type` / `status` / `based_on` / `origin` / `description` / `logo` / `screenshot` | Unknown OS / Unknown status / Independent / Unknown / No description / Not listed / Not listed |
| `app.discover.detail.arch_desktop` | Architecture: {arch}  ·  Desktop: {desktop} |
| `app.discover.detail.direct_isos` | DIRECT ISO FILES |
| `app.discover.detail.loading` | Loading… |
| `app.discover.detail.open_page` | Open DistroWatch download page |
| `app.discover.detail.download_use` | Download & use ISO |
| `app.discover.artwork.loading` / `none` / `unavailable` | Loading artwork… / No artwork / Artwork unavailable |
| `app.discover.omarchy_mx.title` / `.body` / `.badge` | Omarchy MX Mac · Apple Silicon derivative / Installs onto an existing Asahi Arch Minimal system; its releases contain signed installer files, not an ISO/IMG. / Not writable to USB |
| `app.pi.title` | Official Raspberry Pi Imager catalog |
| `app.pi.subtitle` | Board compatibility, compressed and extracted checksums included |
| `app.pi.board_filter` / `compatible_images` | BOARD FILTER / COMPATIBLE IMAGES |
| `app.pi.all_images` | All images |
| `app.pi.empty` / `app.pi.empty_query` | No compatible Raspberry Pi images found / No Raspberry Pi images match “{query}” |
| `app.pi.hint` | Choose a board and image. Official checksums are verified before the image is used. |
| `app.pi.default_category` / `date_unknown` | Raspberry Pi image / Date unknown |
| `app.pi.sizes` | Download {download} · Expanded {expanded}\nReleased {date}\n{description} |
| `app.pi.download_use` | Download, verify & use |

### Downloads panel

| Key | English |
| --- | --- |
| `app.downloads.title` | Downloads |
| `app.downloads.subtitle` | Persistent history · interrupted transfers can resume |
| `app.downloads.empty` | No managed downloads yet (TUI: `No managed downloads yet · choose an image from Discover to begin`; differ) |
| `app.downloads.interrupted_note` | Interrupted transfers retain only owned partial files; explicit cancellation removes them. |
| `app.downloads.action.pause` / `resume` / `cancel` / `cancelling` / `retry` / `remove` | Pause / Resume / Cancel / Cancelling… / Retry / Remove |

### Status line messages (identical or near-identical in both apps)

These are assigned to `self.status` in both apps. Where wording differs between
GUI and TUI the GUI form is listed second; pick one when extracting.

| Key | English |
| --- | --- |
| `app.status.image.busy` | Image inspection is already running |
| `app.status.image.cancelled` | Image selection cancelled |
| `app.status.image.inspecting` | Inspecting image • compressed sources are measured after expansion… |
| `app.status.image.recognized` | Recognized {kind} |
| `app.status.image.stopped` | Image inspection stopped unexpectedly |
| `app.status.image.folder` | Image browser folder: {path} |
| `app.status.image.folder_cancelled` | Folder selection cancelled |
| `app.status.image.choose_first` | Choose an image first |
| `app.status.image.no_recent` | No recent image in that position |
| `app.status.prefs.save_failed` | Preferences were not saved: {error} |
| `app.status.target.none_eligible` | No eligible removable drive is available |
| `app.status.target.selected` | Target selected · confirm the physical drive before reviewing the erase plan |
| `app.status.target.blocked` | That drive is blocked and cannot be selected |
| `app.status.target.choose_first` | Choose a target drive first / No target device is selected |
| `app.status.drives.refresh_paused` | Drive refresh is paused while writing • do not unplug the target |
| `app.status.drives.up_to_date` | Drive list is up to date • automatic detection is on |
| `app.status.drives.changed` | Drive details changed • list updated automatically |
| `app.status.drives.added` | Detected {count} new drive(s) • list updated automatically (plural) |
| `app.status.drives.removed` | Removed {count} drive(s) • list updated automatically (plural) |
| `app.status.drives.added_removed` | Drive list changed: {added} added, {removed} removed • updated automatically |
| `app.status.review.start_with_image` | Start with --image /path/to/image.iso to create a plan (TUI) |
| `app.status.review.open` | Reviewing the write plan • nothing has been written |
| `app.status.review.consequences` | Review the target changes and consequences before writing |
| `app.status.review.ack_required` | Acknowledge the consequences before confirming the write |
| `app.status.write.active` | Writing is active • do not close the app or unplug the target |
| `app.status.write.active_stop_hint` | Writing is active • press x to stop safely; do not unplug the target (TUI) |
| `app.status.write.started` | Write started • do not unplug the target |
| `app.status.write.cancelled` | Write cancelled before erasure • the target is unchanged |
| `app.status.write.stopping` | Stopping safely • flushing completed writes; media will remain incomplete (GUI: `...; the media will remain incomplete`) |
| `app.status.write.close_while_cancelling_download` | Cancelling download safely • close again after temporary data is cleaned up (GUI) |
| `app.status.write.close_while_stopping` | Stopping write safely • close again after completed writes are flushed (GUI) |
| `app.status.catalog.closed` | Catalog closed • {guidance} |
| `app.status.catalog.search_hint` | Type to search · results update live · Esc leaves search |
| `app.status.catalog.search_closed` | Search closed · showing DistroWatch six-month popularity |
| `app.status.catalog.popularity` | Showing DistroWatch six-month popularity |
| `app.status.catalog.ready` | DistroWatch catalog ready • rankings indicate interest, not quality |
| `app.status.catalog.loading_popularity` | Loading DistroWatch six-month popularity… |
| `app.status.catalog.searching_directory` | Searching DistroWatch's full distribution directory… |
| `app.status.catalog.loading` | Loading distributions… |
| `app.status.catalog.loading_base` | Searching DistroWatch for active {base}-based distributions… (TUI: `Loading {base}-based distributions…`) |
| `app.status.catalog.showing_base` | Showing {count} active {base}-based distributions from DistroWatch |
| `app.status.catalog.loading_releases` | Loading {name} releases… / Resolving current {name} ISO files… |
| `app.status.catalog.profile_ready_no_iso` | Profile ready · no direct ISO found / Profile loaded • no direct ISO was resolved from its current links |
| `app.status.catalog.profile_ready_errors` | Profile ready · no direct ISO found · {count} source error(s) (plural) |
| `app.status.catalog.omarchy_enter` | Omarchy quick access · press Enter to resolve ISOs |
| `app.status.catalog.omarchy_missing` | Omarchy is missing from the current DistroWatch directory |
| `app.status.catalog.omarchy_family` | Omarchy family · ISO releases are writable; installer-only derivatives are clearly marked |
| `app.status.catalog.windows_tools` | Windows media tools · press o to choose a Windows ISO (GUI: `... · choose a Windows ISO to unlock setup options`) |
| `app.status.catalog.windows_uses_iso` | Windows tools use the selected local ISO |
| `app.status.catalog.pi_selected` | Raspberry Pi image discovery selected |
| `app.status.catalog.pi_loading` | Loading Raspberry Pi images… / Loading the official Raspberry Pi Imager catalog… |
| `app.status.catalog.pi_compatible` | Showing images compatible with {board} |
| `app.status.catalog.pi_all` | Showing every Raspberry Pi image |
| `app.status.catalog.pi_choose` | Choose a Raspberry Pi image first / Choose a Raspberry Pi image to download |
| `app.status.catalog.pi_selected_verify` | Raspberry Pi image selected • download will be verified (GUI: `... will be extracted and verified`) |
| `app.status.catalog.iso_selected` | ISO selected • choose Download & use ISO (GUI: `ISO selected • publisher checksum will be verified before use` / `... unavailable; HTTPS length and boot structure will be checked`) |
| `app.status.catalog.choose_distribution` | Choose a distribution first |
| `app.status.catalog.choose_release` | Choose an ISO release first / Choose an ISO release to download |
| `app.status.catalog.browser_opened` | Opened the DistroWatch distribution page in your browser |
| `app.status.catalog.artwork_error` | Could not decode catalog artwork: {error} |
| `app.status.download.history_unavailable` | Download history unavailable · {error} |
| `app.status.download.queued` | Download queued · it starts when the active job finishes (GUI: `... it will start when the active job finishes`) |
| `app.status.download.retrying` | Retrying download · preserved bytes resume when supported (GUI: `... will be resumed when supported`) |
| `app.status.download.starting` | Starting managed download… |
| `app.status.download.retry_queued` | Retry queued · it starts after the active download (GUI: `it will start after`) |
| `app.status.download.choose_job` | Choose a download job first |
| `app.status.download.job_gone` | Download job no longer exists (GUI) |
| `app.status.download.start_failed` | Could not start queued download · {error} |
| `app.status.download.using_completed` | Using completed download {path} |
| `app.status.download.completed_unavailable` | Downloaded image is unavailable · {error} |
| `app.status.download.history_removed` | History entry removed · completed image kept (GUI: `Download history entry removed · completed image kept`) |
| `app.status.download.iso_cancelled` | ISO download cancelled |
| `app.status.download.pi_cancelled` | Raspberry Pi image download cancelled |
| `app.status.download.ready` | Ready · downloaded, verified, and inspected {name} · discovery remains open |
| `app.status.download.cancelled_cleaned` | Download cancelled • temporary data cleaned up |
| `app.status.download.stopped` | Download stopped · {error} |
| `app.status.download.paused` | Download paused • press p to resume or x to cancel (GUI: `... • resume or cancel when ready`) |
| `app.status.download.resumed` | Download resumed |
| `app.status.download.cancelling` | Cancelling download safely • cleaning temporary data… |
| `app.status.windows.choose_image` | Choose a Windows image before changing Windows options |
| `app.status.windows.not_windows` | Windows setup options apply only to Windows installer images |
| `app.status.windows.choose_installer` | Choose a Windows installer image before changing Windows options |
| `app.status.windows.updated` | Windows installer selections updated (GUI) / Windows QoL policy selection updated (TUI) |
| `app.status.windows.bypass_on` / `off` | Windows 11 TPM, Secure Boot, and RAM checks will be bypassed / Windows 11 hardware checks use Microsoft defaults |
| `app.status.windows.local_on` / `off` | Windows OOBE will expose the offline/local-account path / Windows OOBE will use its standard account flow |
| `app.status.windows.privacy_on` / `off` | Windows OOBE will use privacy-focused defaults / Windows OOBE privacy questions will remain at their defaults |
| `app.status.windows.bitlocker_on` / `off` | Automatic Windows device encryption will be disabled / Windows may automatically enable device encryption |
| `app.status.windows.account_off` | Automatic local-account creation disabled |
| `app.status.windows.account_on` | Windows will create local administrator account `{account}` |
| `app.status.windows.region_off` / `on` | Windows Setup will ask for regional options / Windows will use locale {locale} and time zone {zone} |
| `app.status.windows.ca2023` | CA 2023 boot media requires updated Secure Boot certificates (GUI: `CA 2023 media requires updated Secure Boot firmware certificates`) |
| `app.status.windows.skusipolicy` | SkuSiPolicy.p7b selection updated |
| `app.status.windows.smode` | S Mode may remain enforced after reinstall; review the plan carefully (GUI: `... review before writing`) |
| `app.status.options.expanded` / `collapsed` | Advanced options expanded • every option is included in the reviewed plan (GUI: `every choice`) / Advanced options collapsed • configured values remain active |
| `app.status.options.open_needs_image` | Choose or download an image before opening media options |
| `app.status.checksum.algorithm` | Checksum algorithm: {algorithm} |
| `app.status.checksum.choose_image` | Choose an image before computing its checksum |
| `app.status.badblocks.off` / `n` | Destructive bad-block check disabled / Bad-block check: {passes} destructive pattern(s) before writing (plural) |
| `app.status.backup.choose_drive` | Choose a removable drive to back up |
| `app.status.backup.cancelled` | Drive backup cancelled |
| `app.status.backup.running` | Backing up {drive}… (GUI: `Backing up {drive} in the background…`) |
| `app.status.backup.done` | Drive image saved to {path} |

Wording differences marked above ("GUI/TUI differ") are existing parity drift;
resolve them by choosing one phrasing per key rather than keeping two keys.

### CLI output (`bootable` subcommands, TUI binary)

CLI output is partly machine-oriented. Decide per line whether it is prose
(translate) or a field name scripts rely on (keep stable and English, or make
output locale-independent with a `--json`-style mode). Candidate keys:

| Key | English |
| --- | --- |
| `cli.about` | Inspect, plan, and safely write boot media |
| `cli.popularity.heading` | DistroWatch popularity · six-month page-hit ranking |
| `cli.popularity.disclaimer` | Interest indicator only; not usage, quality, or market share. |
| `cli.release_date` | DistroWatch release date: {date} |
| `cli.ready_to_write` | Ready to write: {path} |
| `cli.field.image` / `kind` / `size` / `warning` | Image / Kind / Size / Warning |
| `cli.pi.heading` | Raspberry Pi Imager catalog · {count} image(s) |
| `cli.pi.out_of_range` | Pi image index {index} is out of range |
| `cli.download.kept` | Image kept at {path}; pass that path instead of the slug to skip the download. |
| `cli.done` | Done: {image} written and verified on {device} |
| `cli.plan.field.source` / `target` / `strategy` / `confirmation` | Source / Target / Strategy / Confirmation |
| `cli.plan.erases` | ERASES DATA |

### Core strings not yet converted

These originate in `bootable-core` and still produce English. They are the next
conversions to make there (add `Message` variants, then adapters pick them up
for free):

* `Progress.message` text throughout `lib.rs`, `download.rs`, `catalog.rs`,
  `pi_catalog.rs`, `write_session.rs`, `platform/*` (for example `Stage 5/5 ·
  Inspecting boot structure and media strategy`, `Ready · downloaded ...`).
  Because `Progress` carries a `String`, either add a typed `ProgressMessage`
  alongside it or keep the string and add a `kind` the adapter can localize.
* `WritePlan.steps[*].title` in `plan.rs` (for example `Unmount target
  filesystems`). Plan steps need to be generated per locale or carried as a
  `Message` plus arguments.
* `Error` `Display` text in `error.rs` (`thiserror` messages). Keep the English
  `Display` for logs and add `Error::message(locale)`.
* `ImageKind`, `WriteStrategy`, `WindowsPartitionScheme`, `BadBlockCheck`
  `Display` impls in `model.rs`; `Progress::metrics` (`remaining`, `elapsed`).
* The `signature_note` reasons inside `IntegrityState::ChecksumVerified`
  (`the publisher does not publish a signature`, ...) are English strings
  stored in data; make them a typed enum before localizing them. The phrasing
  around the note is already translated.
* `DownloadJob` row labels (`download.rs`).

## Open questions for review

* Is a hand-rolled catalog acceptable long term, or should the format become
  real Fluent (`.ftl`) so translators can use standard tooling (Weblate,
  Pontoon)? The key names and placeholder syntax were chosen to make that
  mechanical.
* Hindi: ship a reviewed translation or keep it listed-but-hidden?
* Should the confirmation-phrase prompt text be localized while the phrase
  stays literal? (Recommended: yes.)
