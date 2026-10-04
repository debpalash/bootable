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
| `Strings` (a `Locale` bound to the catalog) | `crates/bootable-core/src/messages.rs`, `Locale::strings()` |
| Converted core strings | help guide, drive detail rows, target eligibility, removable-media status, readiness/guidance/status text, step titles, progress phase names, integrity labels, download status/kind labels, catalog load states, write completion status/result text, bad-block labels |
| Shared app strings (the text both interfaces write themselves) | see [Shared app strings](#shared-app-strings): 371 keys grouped by screen |

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

Strings                        Copy handle: Locale + catalog (Locale::strings())
  t.text(m) / t.format(m, args) / t.plural(m, count, args)
  t.heading(m)                 upper-cased text for headings and badges
  t.locale()

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
  DownloadStatus::label_in(locale), DownloadKind::label_in(locale)
  CatalogState::short_label_in(locale, subject)
  CatalogFetch::{source_label_in, status_suffix_in}(locale)
  BadBlockCheck::{label_in, status_in}(locale)
  WriteCompletion::{status_in, title_in, detail_in}(locale)
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
| `en` | English | Source of truth, complete (457/457) |
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

* the erase/safety wording in `help.intro`, `help.target.choose.detail`, the
  `workspace.*` / `readiness.*` strings, and the shared app strings
  `confirm.*`, `review.*`, `status.write.*`, `result.*`,
  `target.confirm_physical` and `status.target.*`, which tell people nothing
  has been written yet or what an irreversible step will do; a mistranslation
  here is a safety problem, not a style problem;
* the Windows setup option labels and their on/off status lines
  (`options.windows.*`), which describe real installer behavior;
* the catalog load-state phrasing (`catalog.state.*`), written as label-style
  sentences (`Nicht geladen: {subject}`) to dodge gender/number agreement;
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
* No message, in any locale, contains `ERASE ` or takes a confirmation-phrase
  placeholder.
* Technical tokens (`SHA-256`, `SkuSiPolicy.p7b`, `autounattend.xml`,
  `BitLocker`, `DistroWatch`, `Raspberry Pi`, `HTTPS`, `UEFI`, `MBR`, `TPM`,
  `Copilot`, `OneDrive`, `Teams`, `CA 2023`) that appear in the English text
  appear in every translation.
* The English output of the localized helpers (`DownloadStatus::label_in`,
  `CatalogState::short_label_in`, `WriteCompletion::status`, ...) equals the
  legacy `Display`/`short_label`/`status` text.

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
  it. The catalog never contains it: no message, in any locale, may include
  `ERASE ` or take a confirmation-phrase placeholder (tests enforce this), and
  neither interface asks the user to type it.
* Device paths, device ids, serials, file names, checksum algorithm names
  (`SHA-256`), key fingerprints, URLs, command-line flags and the privileged
  helper protocol.
* Key chords (`Ctrl+O`, `Esc`, `↑ ↓ · j k`). Only descriptions of inputs such
  as "Stop button" are translated. Shared strings that mention a shortcut take
  it as a placeholder (`tooltip.guide`: `{shortcut}`).
* Technical identifiers rendered inside translated sentences: partition scheme
  and firmware values (`GPT`, `MBR`, `UEFI`, `BIOS + UEFI (CSM)`), `SkuSiPolicy.p7b`,
  `autounattend.xml`, and the other tokens listed under "Per-key notes".
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
* Arrows are directional and need a mirrored variant per direction. The shared
  app strings already keep them out of translatable text (`action.select` is
  `Select`, not `Select →`; see "Conventions"), so the adapter draws and mirrors
  them. The only arrow left inside catalog text is the `Source → Target →
  Review & write` flow in `help.intro`.

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

## Shared app strings

Everything both interfaces write themselves (headers, buttons, panel bodies and
empty states, Windows option labels and hints, the review and confirmation
dialogs, download rows, status lines) now lives in the catalog, so the GUI and
TUI cannot drift apart again. The catalog was extracted from
`apps/bootable-desktop/src/main.rs` and `apps/bootable-tui/src/main.rs`; where
the two worded one concept differently, one wording was chosen (see
[Unification decisions](#unification-decisions)).

Adapters call `Message::X.text(locale)`, `.format(locale, &[...])`,
`.plural(locale, count, &[...])` or, to avoid threading the locale, a
`Strings` bound to it:

```rust
let t = preferences.locale().strings();          // Copy; rebuild on language change
let label = t.text(Message::ActionBrowse);
let line  = t.format(Message::StatusBackupDone, &[("path", &destination.display())]);
let line  = t.plural(Message::StatusDrivesAdded, added as u64, &[]);
let badge = t.heading(Message::ReviewStepErases);   // "ERASES DATA"
```

### Conventions

* **Names**: `<screen>.<thing>[.<variant>]`, snake case, dot separated. Screens:
  `header`, `action`, `tooltip`, `guide`, `source`, `target`, `focus`,
  `options`, `review`, `confirm`, `result`, `discover`, `pi`, `catalog`,
  `downloads`, `status`, `common`. The Rust variant is the CamelCase of the key
  (`status.drives.added` is `Message::StatusDrivesAdded`).
* **Sentence case.** Catalog text is stored in sentence case. Section headings
  and badges that were upper case (`SOURCE`, `RECENT IMAGES`, `SELECTED DRIVE`,
  `ERASES DATA`, `PERMANENT`, `DIRECT ISO FILES`) are the same keys rendered
  with `Strings::heading` (Rust `to_uppercase`; caseless scripts are
  unchanged). Do not store shouting text in a translation.
* **No glyphs or key chords in text.** `→ ✓ × ⇩ ↻ ›` and similar are drawn by the
  adapter (`Select` + `→`). Shortcuts are injected: `tooltip.guide` takes
  `{shortcut}` (`F1` in the GUI, `?` in the TUI).
* **Width variants.** `*_compact`, `*.short` and `*_value_compact` are
  narrow-layout forms of the *same* concept. Pick by available width; never
  use a different wording elsewhere. The long form is canonical.
* **Label + value** uses `common.labeled` (`{label}: {value}`), which fixes
  spacing rules per language (the French table puts a space before the colon).
* **Counts** are always plural messages (`.plural(...)`, `{count}` implicit),
  never `(s)`.
* **External text** (`{error}`, `{path}`, `{kind}`, `{name}`, `{description}`,
  `{warning}`, `{reason}`, `{source}`) is shown as received; see
  [Per-key notes](#per-key-notes-verbatim-and-external-text).

### Core helpers that use these keys

These return catalog text and keep the English output of their predecessor
byte-for-byte (tests pin it):

| Helper | Replaces |
| --- | --- |
| `DownloadStatus::label_in(locale)` | `Display` of the status in download rows |
| `DownloadKind::label_in(locale)` | `Display` of the download kind |
| `CatalogState::short_label_in(locale, subject)` | `short_label` (pass `Message::CatalogSubject*` text as `subject`) |
| `CatalogFetch::source_label_in` / `status_suffix_in(locale)` | `source_label` / `status_suffix` (the `{source}` argument) |
| `BadBlockCheck::label_in(locale)` / `status_in(locale)` | "Bad blocks off / 2x" control label and the bad-block status line |
| `WriteCompletion::status_in` / `title_in` / `detail_in(locale)` | `status()` and the hand-written result panel title/body in both apps |
| `Locale::strings()` | threading `locale` through every call |

### Unification decisions

Where the GUI and TUI differed, the clearer wording was chosen for both. "GUI"
and "TUI" are the wordings before this change; the last column is the key every
adapter now uses.

| Concept | GUI said | TUI said | Chosen |
| --- | --- | --- | --- |
| Brand tagline | `Boot media, written deliberately.` | (none) | `header.tagline` in both, next to the brand when there is room |
| Create-screen subtitle | `Image → removable drive → verified result` | `One deliberate path from image to removable drive.` | TUI wording, `header.subtitle.create`; the GUI flow line is dropped |
| Review subtitle | header `Inspect every operation before confirmation.`; card `Nothing is written until a separate destructive confirmation succeeds.` | `Nothing is written until the consequences are reviewed and acknowledged.` | header: `header.subtitle.review`; review panel: `review.subtitle` (TUI wording), `review.subtitle_writing` while writing |
| Downloads button | `Downloads · n` / `Jobs · n` | `Downloads` / `Jobs` | `action.downloads` (`_count` when a number is shown, `_compact` = Jobs) |
| Catalog toggle | `Catalog ×` / `Close catalog` | `× Catalog` / `× Cat` | `action.catalog_close` (`_compact` = Catalog); the `×` is drawn by the adapter |
| Discover button | `Discover images` | `Discover` / `Find` | `action.discover` (`_compact` = Find) |
| Setup toggle | `Setup options` / `Hide options` / `Options` | `Setup options` / `Hide options` / `Setup` / `Hide` | `action.setup_options` / `action.hide_options` (+ `_compact` Setup / Hide); `Options` is dropped |
| Refresh drives | tooltip `Refresh removable drives` | `Refresh` / `USB` | label `action.refresh_drives` (same words as the Guide), tooltip `tooltip.refresh_drives`, narrow label `action.refresh` |
| Select / Selected / Blocked | `Select →` / `Selected` / `Blocked` | same | `action.select` / `action.selected` / `action.blocked`; arrow drawn by adapter |
| Source panel body | `ISO, IMG, RAW, or compressed disk image` + hint `The image is inspected before any write is allowed` | `ISO, IMG, … image` + `Inspected before writing` | `source.formats` + `source.hint`; `source.inspected` for the checked state |
| Inspecting button | `Inspecting…` | `…  Inspecting` | `action.inspecting` |
| Physical-drive reminder | `Confirm the physical drive before continuing · erasure starts only after review` | `Confirm the physical drive · erasure starts only after review` | GUI wording, `target.confirm_physical` |
| No target chosen | `Choose a target drive first` | `No target device is selected` | `status.target.choose_first` (GUI) |
| Windows option labels | long sentence (`Bypass TPM, Secure Boot and RAM checks`) | short caption (`Hardware bypass`) | `options.windows.<id>.label` is canonical; `.short` is its compact caption (both are in the catalog, one concept) |
| Offline account caption | n/a | `Local account` (setup panel) and `Offline account` (Windows card) | `Offline account` (`options.windows.offline_account.short`); `Named account` is the other one |
| Windows toggle feedback | `Windows installer selections updated` (generic) | specific on/off lines for six options, generic for QoL / CA 2023 / SkuSiPolicy / S Mode | every option has its own `.on` / `.off` line (`options.windows.<id>.on/off`), shown by both |
| Scheme / firmware status | `… partition scheme: {} · target firmware: {}` | `… partition scheme: {} · boot firmware: {}` and `Boot firmware: {} (experimental) · partition scheme: {}` | "boot firmware" everywhere: `status.windows.scheme`, `status.windows.firmware` |
| Linux always-on options | two long checkboxes | four short checkboxes | both show the same four items: `options.linux.layout` + `options.linux.verify` (long) and the `*_short` captions; the TUI uses captions where it is narrow |
| Bad blocks control | `Bad blocks off` / `Bad blocks 2x` | `Bad blocks: off` / `Bad blocks: 2x` | no colon (`options.tools.bad_blocks_off` / `_passes`), via `BadBlockCheck::label_in` |
| Review consequence | `…will be erased` (no period) | `…will be erased.` | with period, `review.consequence` |
| Write result body | `The image was written and verified. …` | `Image written and verified. …` | GUI wording, `result.success.body` |
| Confirmation bullets | four long sentences | four different sentences plus a three-bullet compact variant | the four GUI sentences (`confirm.consequence.*`); the TUI compact mode shows the first two |
| Confirmation buttons | `Cancel` / `Confirm erase & write` | `Cancel · target unchanged` / `Acknowledge first` (disabled) | `action.cancel` (the status line already says the target is unchanged), `confirm.submit`, and `confirm.acknowledge_first` as the disabled-state label in both |
| Review buttons | `Back` | `Back to selection` / `Back locked`; `Quit locked` | `action.back`, `action.quit`; locked state is the disabled style, not a different label |
| Download row actions | `Retry` / `Use` / `Remove` | `Retry / resume` / `Use image` / `Remove entry` | TUI wording (`downloads.action.*`); the active-download button is `downloads.action.cancel` (`Cancel download`) in both |
| Download empty state | `No managed downloads yet` | `… · choose an image from Discover to begin` | TUI wording, `downloads.empty` |
| Discover title | `Discover distributions` | `Discover bootable images` | `discover.title` (TUI) |
| DistroWatch disclaimer | `…measures interest—not quality or market share` | long `six-month page-hit ranking + Interest indicator only; not usage, quality, or market share.` | short sentence, `discover.disclaimer` |
| Unknown origin / date | `Unknown origin` / `Date unknown` | `Unknown` / `unknown` | `discover.detail.unknown_origin` ("Unknown", the `Origin` label gives context) and `pi.date_unknown` |
| Status: download queued / retrying / retry queued / history removed / paused | `…it will start…`, `…will be resumed…`, `Download history entry removed`, `…resume or cancel when ready` | `…it starts…`, `…resume when supported`, `History entry removed`, `…press p to resume or x to cancel` | GUI wording without key legends (`status.download.*`); a TUI that wants the key legend appends it itself |
| Status: catalog loading | `Searching DistroWatch for active {base}-based…`, `Resolving current {name} ISO files…` | `Loading {base}-based distributions…`, `Loading {name} releases…` | GUI wording |
| Status: profile with no ISO | `Profile loaded • no direct ISO was resolved from its current links` | `Profile ready · no direct ISO found` | TUI wording (shorter, consistent with the error variant) |
| Status: ISO / Pi selected | `ISO selected • publisher checksum will be verified before use` / `…unavailable; HTTPS length…`; `…will be extracted and verified` | `ISO selected • choose Download & use ISO`; `…download will be verified` | GUI wording (two checksum-aware ISO lines) |
| Status: backup running | `Backing up {} in the background…` | `Backing up {}…` | neutral `Backing up {drive}…` (the TUI backup blocks) |
| Status: options toggled | `…every choice is included…` | `…every option is included…` | `every choice` |

### Per-key notes (verbatim and external text)

* **The erase confirmation phrase is never in the catalog.** No message
  contains `ERASE ` or a `{phrase}`/`{confirmation}` placeholder (a test
  enforces both). The GUI and TUI confirm with an acknowledgement
  checkbox (`confirm.ack`), not typed text; the CLI `flash` / `write` commands
  print and compare `plan.confirmation_phrase` themselves. `confirm.*` and
  `review.*` text surrounds the destructive action, it never replaces it.
* **Safety-critical wording to prioritise in native review**: `confirm.*`,
  `review.consequence`, `review.warning.*`, `review.state.*`,
  `status.write.*`, `result.*`, `target.confirm_physical`, `status.target.*`,
  `focus.review`.
* **Technical tokens survive translation verbatim** (enforced by a test over
  every locale): `SHA-256`, `SkuSiPolicy.p7b`, `autounattend.xml`, `BitLocker`,
  `DistroWatch`, `Raspberry Pi`, `HTTPS`, `UEFI`, `MBR`, `TPM`, `Copilot`,
  `OneDrive`, `Teams`, `CA 2023`. German may hyphenate a brand into a compound
  (`Raspberry-Pi-Katalog`).
* **Partition scheme and firmware values** (`GPT`, `MBR`, `UEFI`, `BIOS + UEFI
  (CSM)`) come from `Display` of `WindowsPartitionScheme` /
  `WindowsBootFirmware`. They are technical identifiers, never localized, and
  arrive as `{scheme}`, `{firmware}`, `{value}`. The GUI's select matches rows
  by `to_string()`, so they must stay as they are.
* **User and host data**: `{account}` / `{name}` (suggested local account
  name; the fallback `User` is adapter-local), `{locale}` and `{zone}` (host
  locale and time zone), `{path}`, `{drive}`, `{board}`, `{query}`,
  `{description}`.
* **Core text still shown as received** because core has not been converted
  yet: `{kind}` in `status.image.recognized` (`ImageKind` `Display`), `{error}`
  everywhere (core `Error` text), `{step}` in `status.backup.failed`
  (`Progress.message`), `{name}` of distributions, and the `{reason}` /
  `{warning}` inside `catalog.state.*` which *are* localized
  (`catalog.failure.*`).
* **`{source}`** in `status.catalog.*_loaded` is
  `CatalogFetch::source_label_in(locale)` (`catalog.source.*`).
* **`status.download.ready` starts with `Ready ·` in English.** Both apps test
  `status.starts_with("Ready ·")` so a later status does not overwrite it. That
  comparison breaks as soon as the text is localized; replace it with a state
  flag (for example "download finished, keep status until the next user
  action") before wiring this key.
* **`catalog.subject.*`** are nouns fed into `catalog.state.*` through
  `CatalogState::short_label_in`. English keeps them lower case as today
  (`distributions not loaded`); other tables use label-style sentences
  (`Nicht geladen: {subject}`) to avoid gender and number agreement problems.
* `options.windows.boot_firmware_hint` states the feature is experimental and
  unverified on real hardware; keep that disclaimer in every translation.
* `discover.item.hits_per_day` is `{hits}/day`; `{hits}` is a number the adapter
  formats.
* `options.windows.named_account.on` quotes the account name in backticks;
  keep them (they delimit user data).

### Adapter migration notes

* Rebuild `Strings` (and clear or re-render cached `String` statuses) when the
  language changes. Statuses that carry user data are best stored as a
  `Message` plus arguments and rendered at draw time.
* Several statuses are assigned by core objects (`ReviewedWriteSession::begin`
  errors, `apply_progress`, `Progress.message`); those are listed under "Core
  strings not yet converted" below and remain English for now.
* The GUI should stop hand-building the three Windows scheme/firmware strings
  and the per-checkbox statuses; the TUI's `panel_heading` English-only
  qualifier (` · choose an image`, ` · removable media`) can show
  `source.title` / `target.title` instead of dropping the qualifier in
  non-English locales.

### Key list

Every key below exists in all shipped locales. Plural keys are marked; the
table shows the English `other` form. `{placeholders}` are identical in every
translation.

#### Header and brand

| Key | English |
| --- | --- |
| `header.tagline` | Boot media, written deliberately. |
| `header.title.create` | Create boot media |
| `header.subtitle.create` | One deliberate path from image to removable drive. |
| `header.subtitle.review` | Inspect every operation before confirmation. |
| `guide.title` | Guide |
| `common.labeled` | {label}: {value} |

#### Buttons and actions shared by both interfaces

| Key | English |
| --- | --- |
| `action.downloads` | Downloads |
| `action.downloads_count` | Downloads · {count} |
| `action.downloads_compact` | Jobs |
| `action.discover` | Discover images |
| `action.discover_compact` | Find |
| `action.catalog_close` | Close catalog |
| `action.catalog_close_compact` | Catalog |
| `action.setup_options` | Setup options |
| `action.setup_options_compact` | Setup |
| `action.hide_options` | Hide options |
| `action.hide_options_compact` | Hide |
| `action.refresh_drives` | Refresh drives |
| `action.refresh` | Refresh |
| `action.retry` | Retry |
| `action.browse` | Browse |
| `action.change` | Change |
| `action.inspecting` | Inspecting… |
| `action.select` | Select |
| `action.selected` | Selected |
| `action.blocked` | Blocked |
| `action.close` | Close |
| `action.cancel` | Cancel |
| `action.cancelling` | Cancelling… |
| `action.back` | Back |
| `action.pause` | Pause |
| `action.resume` | Resume |
| `action.stop` | Stop |
| `action.review` | Review |
| `action.quit` | Quit |

#### Tooltips

| Key | English |
| --- | --- |
| `tooltip.guide` | Guide and shortcuts ({shortcut}) |
| `tooltip.refresh_drives` | Refresh removable drives |
| `tooltip.refresh_distrowatch` | Refresh DistroWatch data |
| `tooltip.refresh_pi` | Refresh Raspberry Pi catalog |

#### Source panel

| Key | English |
| --- | --- |
| `source.title` | Choose an image |
| `source.hint` | The image is inspected before any write is allowed |
| `source.formats` | ISO, IMG, RAW, or compressed disk image |
| `source.inspected` | Inspected |
| `source.recent.title` | Recent images |
| `source.recent.empty` | Images you use appear here for one-click reuse |
| `source.recent.in_use` | In use |
| `source.dialog.title` | Boot images |
| `source.dialog.filter_iso` | ISO images |
| `source.dialog.filter_backup` | Raw drive image |

#### Target panel

| Key | English |
| --- | --- |
| `target.title` | Choose a drive |
| `target.empty` | Connect a removable USB or SD drive, then refresh |
| `target.selected_drive` | Selected drive |
| `target.confirm_physical` | Confirm the physical drive before continuing · erasure starts only after review |

#### Workspace focus guidance (shown when keyboard focus moves)

| Key | English |
| --- | --- |
| `focus.source` | Source · choose or change the image |
| `focus.target` | Target · choose an eligible removable drive |
| `focus.setup` | Setup options · configure image-specific choices |
| `focus.review` | Review & write · inspect the plan before erasure |
| `focus.discover` | Discover images · browse trusted catalogs |
| `focus.refresh` | Refresh drives · rescan removable media |

#### Setup options: panel titles

| Key | English |
| --- | --- |
| `options.windows.title` | Windows installer options |
| `options.linux.title` | Linux / Unix boot media |
| `options.selected_count` | {count} selected |
| `options.summary.verification` | Verification on · {bad_blocks} |

#### Setup options: partition scheme and boot firmware (GPT, MBR, UEFI and BIOS + UEFI (CSM) stay verbatim)

| Key | English |
| --- | --- |
| `options.windows.partition_scheme` | Partition scheme |
| `options.windows.scheme_value` | Scheme: {scheme} |
| `options.windows.boot_firmware` | Boot firmware |
| `options.windows.boot_firmware_experimental` | Boot firmware · experimental |
| `options.windows.boot_firmware_value` | Boot firmware: {value} |
| `options.windows.boot_firmware_value_experimental` | Boot firmware: {value} · experimental |
| `options.windows.boot_firmware_value_compact` | Firmware: {value} |
| `options.windows.boot_firmware_hint` | Experimental: BIOS + UEFI (CSM) needs the MBR scheme and is currently written only by the Linux adapter. Not yet verified on real hardware. |

#### Setup options: Windows checkboxes (label = full wording; short = compact cells; on/off = status line when toggled)

| Key | English |
| --- | --- |
| `options.windows.bypass_hardware.label` | Bypass TPM, Secure Boot and RAM checks |
| `options.windows.bypass_hardware.short` | Hardware bypass |
| `options.windows.bypass_hardware.on` | Windows 11 TPM, Secure Boot, and RAM checks will be bypassed |
| `options.windows.bypass_hardware.off` | Windows 11 hardware checks use Microsoft defaults |
| `options.windows.offline_account.label` | Expose local/offline account setup |
| `options.windows.offline_account.short` | Offline account |
| `options.windows.offline_account.on` | Windows OOBE will expose the offline/local-account path |
| `options.windows.offline_account.off` | Windows OOBE will use its standard account flow |
| `options.windows.named_account.label` | Create local account: {name} |
| `options.windows.named_account.short` | Named account |
| `options.windows.named_account.on` | Windows will create local administrator account `{account}` |
| `options.windows.named_account.off` | Automatic local-account creation disabled |
| `options.windows.host_region.label` | Copy this computer's locale and time zone |
| `options.windows.host_region.short` | Host region |
| `options.windows.host_region.on` | Windows will use locale {locale} and time zone {zone} |
| `options.windows.host_region.off` | Windows Setup will ask for regional options |
| `options.windows.privacy.label` | Apply privacy-focused OOBE defaults |
| `options.windows.privacy.short` | Privacy defaults |
| `options.windows.privacy.on` | Windows OOBE will use privacy-focused defaults |
| `options.windows.privacy.off` | Windows OOBE privacy questions will remain at their defaults |
| `options.windows.bitlocker.label` | Disable automatic BitLocker encryption |
| `options.windows.bitlocker.short` | Disable BitLocker |
| `options.windows.bitlocker.on` | Automatic Windows device encryption will be disabled |
| `options.windows.bitlocker.off` | Windows may automatically enable device encryption |
| `options.windows.qol.label` | QoL: reduce Copilot, OneDrive, Teams, suggestions, and Fast Startup |
| `options.windows.qol.short` | QoL policies |
| `options.windows.qol.on` | Windows will reduce Copilot, OneDrive, Teams, suggestions, and Fast Startup |
| `options.windows.qol.off` | Windows keeps its default Copilot, OneDrive, Teams, suggestions, and Fast Startup behavior |
| `options.windows.ca2023.label` | Use Windows UEFI CA 2023 signed bootloaders |
| `options.windows.ca2023.short` | CA 2023 |
| `options.windows.ca2023.on` | CA 2023 boot media requires updated Secure Boot firmware certificates |
| `options.windows.ca2023.off` | Boot media will use the standard Windows bootloaders |
| `options.windows.skusipolicy.label` | Apply SkuSiPolicy.p7b Secure Boot revocations |
| `options.windows.skusipolicy.short` | SkuSiPolicy |
| `options.windows.skusipolicy.on` | SkuSiPolicy.p7b Secure Boot revocations will be applied |
| `options.windows.skusipolicy.off` | SkuSiPolicy.p7b Secure Boot revocations will not be applied |
| `options.windows.smode.label` | Force Windows S Mode (expert) |
| `options.windows.smode.short` | Force S Mode |
| `options.windows.smode.on` | S Mode may remain enforced after reinstall; review the plan carefully |
| `options.windows.smode.off` | Windows S Mode will not be forced |

#### Setup options: Windows status lines

| Key | English |
| --- | --- |
| `status.windows.choose_installer` | Choose a Windows installer image before changing Windows options |
| `status.windows.not_windows` | Windows setup options apply only to Windows installer images |
| `status.windows.scheme` | Windows partition scheme: {scheme} · boot firmware: {firmware} |
| `status.windows.firmware` | Boot firmware: {firmware} (experimental) · partition scheme: {scheme} |

#### Setup options: Windows installer media card (Discover)

| Key | English |
| --- | --- |
| `options.windows.heading` | Windows installer media |
| `options.windows.installer_ready` | Windows ISO ready · setup choices unlocked |
| `options.windows.installer_locked` | Choose a Windows ISO to unlock setup choices |
| `options.windows.choose_iso` | Choose Windows ISO |
| `options.windows.replace_iso` | Replace Windows ISO |
| `options.windows.unavailable_note` | Unavailable items are not clickable. Existing autounattend.xml files are never overwritten. |
| `options.windows.silent_install_warning` | Silent installation can erase the first disk Windows Setup detects and requires a separate high-friction safety design. |

#### Setup options: Linux / Unix media

| Key | English |
| --- | --- |
| `options.linux.layout` | Preserve the complete bootable disk layout |
| `options.linux.layout_short` | Full disk layout |
| `options.linux.verify` | Verify the written bytes with SHA-256 |
| `options.linux.verify_short` | Byte verification |
| `options.linux.boot_records_short` | Boot records |
| `options.linux.unmount_short` | Safe unmount |

#### Setup options: media tools

| Key | English |
| --- | --- |
| `options.tools.title` | Media tools |
| `options.tools.subtitle` | Verification and backup utilities |
| `options.tools.bad_blocks_off` | Bad blocks off |
| `options.tools.bad_blocks_passes` | Bad blocks {passes}x |
| `options.tools.verify_image` | Verify image |
| `options.tools.image_folder` | Image folder |
| `options.tools.backup_drive` | Back up drive |

#### Review screen

| Key | English |
| --- | --- |
| `review.title` | Review write plan |
| `review.subtitle` | Nothing is written until the consequences are reviewed and acknowledged. |
| `review.subtitle_writing` | Writing and verification are active • do not unplug the target. |
| `review.plan_summary` | Plan summary |
| `review.field.source` | Source |
| `review.field.target` | Target |
| `review.field.method` | Method |
| `review.field.consequence` | Consequence |
| `review.consequence` | All existing data and partitions on the selected target will be erased. |
| `review.permanent_changes` | Permanent changes |
| `review.ordered_operations` | Ordered operations |
| `review.step.erases` | Erases data |
| `review.step.safe` | Safe |
| `review.step.verifies` | Verifies |
| `review.state.writing` | Writing and verification are active |
| `review.state.final_confirm` | One final confirmation is required |
| `review.warning.writing` | Do not close the app, power off, or unplug the target drive. |
| `review.warning.idle` | Review the exact target changes and irreversible consequences before writing. |
| `review.hint.open_confirmation` | Open the confirmation to review changes, consequences, and the physical target. |
| `review.action.stop_safely` | Stop safely |
| `review.action.written` | Written & verified |
| `review.action.retry` | Review & retry |
| `review.action.consequences` | Review consequences |
| `review.status.writing` | Writing and verification are active · do not unplug the target |
| `review.status.complete` | Complete · the written media passed byte verification |
| `review.status.review` | Review the physical target and permanent changes before writing |

#### Confirmation dialog (the dialog never contains text the user must type; the erase phrase stays verbatim and is not a catalog message)

| Key | English |
| --- | --- |
| `confirm.title` | Confirm permanent changes |
| `confirm.subtitle` | Review what Bootable will change and what can go wrong. |
| `confirm.badge` | Permanent |
| `confirm.physical_target` | Physical target |
| `confirm.changes` | Changes to this drive |
| `confirm.consequences` | Consequences |
| `confirm.consequence.erase` | Every existing file and partition on this physical drive will be permanently erased. |
| `confirm.consequence.wrong_drive` | Choosing the wrong drive destroys the data on that drive; confirm its model, path, and capacity below. |
| `confirm.consequence.interrupted` | Power loss, closing the app, or unplugging during writing can leave incomplete and unbootable media. |
| `confirm.consequence.recheck` | Bootable rechecks the target identity immediately before erasure and verifies the result afterward. |
| `confirm.ack` | I checked the physical target and understand that all of its existing data will be permanently erased. |
| `confirm.submit` | Confirm erase & write |
| `confirm.acknowledge_first` | Acknowledge first |

#### Write result

| Key | English |
| --- | --- |
| `result.success.title` | Write complete |
| `result.success.body` | The image was written and verified. The removable drive can now be safely removed. |
| `result.authentication_denied.title` | Write cancelled before erasure |
| `result.authentication_denied.body` | Administrator authentication was cancelled or denied. |
| `result.stopped.title` | Write stopped safely |
| `result.stopped.body` | The media is incomplete and must be rewritten before use. |
| `result.failed.title` | Write failed |

#### Review and write status lines

| Key | English |
| --- | --- |
| `status.review.open` | Reviewing the write plan • nothing has been written |
| `status.review.consequences` | Review the target changes and consequences before writing |
| `status.review.ack_required` | Acknowledge the consequences before confirming the write |
| `status.write.active` | Writing is active • do not close the app or unplug the target |
| `status.write.started` | Write started • do not unplug the target |
| `status.write.cancelled` | Write cancelled before erasure • the target is unchanged |
| `status.write.stopping` | Stopping safely • flushing completed writes; the media will remain incomplete |

#### Discover: catalog shell

| Key | English |
| --- | --- |
| `discover.title` | Discover bootable images |
| `discover.collapsed_hint` | Browse trusted catalogs · Open |
| `discover.disclaimer` | DistroWatch page-hit ranking measures interest, not quality or market share. |
| `discover.search_title` | Search |
| `discover.search_placeholder` | Search by name, slug, or base family… |
| `discover.windows_hint` | Windows installer workflow · select an ISO to unlock every setup checkbox |
| `discover.quick.all` | All |
| `discover.section.popular` | Popular · six months |
| `discover.section.search` | Search results |
| `discover.section.arch` | Arch-based |
| `discover.section.debian` | Debian-based |

#### Discover: distribution rows and profile

| Key | English |
| --- | --- |
| `discover.item.independent` | Independent |
| `discover.item.directory` | Directory |
| `discover.item.directory_source` | DistroWatch directory |
| `discover.item.hits_per_day` | {hits}/day |
| `discover.item.size_unknown` | Size unknown |
| `discover.item.publisher_checksum` | Publisher {algorithm} |
| `discover.item.no_publisher_checksum` | No publisher checksum |
| `discover.item.https_only` | HTTPS only |
| `discover.detail.empty` | Choose a distribution to load its profile and ISO files |
| `discover.detail.unknown_os` | Unknown OS |
| `discover.detail.unknown_status` | Unknown status |
| `discover.detail.unknown_origin` | Unknown |
| `discover.detail.no_description` | No description |
| `discover.detail.not_listed` | Not listed |
| `discover.detail.based_on` | Based on |
| `discover.detail.origin` | Origin |
| `discover.detail.architecture` | Architecture |
| `discover.detail.desktop` | Desktop |
| `discover.detail.logo` | Logo |
| `discover.detail.screenshot` | Screenshot |
| `discover.detail.screenshot_of` | {name} screenshot |
| `discover.detail.rating` (plural) | ★ {rating}/10 · {count} reviews |
| `discover.detail.direct_isos` | Direct ISO files |
| `discover.detail.loading` | Loading… |
| `discover.detail.found` | {count} found |
| `discover.detail.open_page` | Open DistroWatch download page |
| `discover.detail.download_use` | Download & use ISO |
| `discover.artwork.loading` | Loading artwork… |
| `discover.artwork.none` | No artwork |
| `discover.artwork.unavailable` | Artwork unavailable |

#### Discover: Raspberry Pi catalog

| Key | English |
| --- | --- |
| `pi.title` | Official Raspberry Pi Imager catalog |
| `pi.subtitle` | Board compatibility, compressed and extracted checksums included |
| `pi.board_filter` | Board filter |
| `pi.compatible_images` | Compatible images |
| `pi.all_images` | All images |
| `pi.empty` | No compatible Raspberry Pi images found |
| `pi.empty_query` | No Raspberry Pi images match “{query}” |
| `pi.hint` | Choose a board and image. Official checksums are verified before the image is used. |
| `pi.default_category` | Raspberry Pi image |
| `pi.date_unknown` | Date unknown |
| `pi.details` | Download {download} · Expanded {expanded}\nReleased {date}\n{description} |
| `pi.download_use` | Download, verify & use |

#### Discover: catalog load states ({subject} comes from catalog.subject.*; {reason} and {warning} are core-produced text shown as received)

| Key | English |
| --- | --- |
| `catalog.state.idle` | {subject} not loaded |
| `catalog.state.loading` | Loading {subject}… |
| `catalog.state.ready` | {subject} ready |
| `catalog.state.ready_cached` | {subject} ready · cached |
| `catalog.state.ready_warning` | {subject} ready · cached · {warning} |
| `catalog.state.empty` | No {subject} found |
| `catalog.state.failed` | Could not load {subject} · {reason} · retry |
| `catalog.subject.distributions` | distributions |
| `catalog.subject.search_catalog` | search catalog |
| `catalog.subject.pi_images` | Raspberry Pi images |
| `catalog.subject.pi_boards` | Raspberry Pi boards |
| `catalog.subject.base_distributions` | {base}-based distributions |
| `catalog.subject.iso_releases` | ISO releases |
| `catalog.subject.distribution_profile` | distribution profile |

#### Discover: status lines

| Key | English |
| --- | --- |
| `status.catalog.closed` | Catalog closed • {guidance} |
| `status.catalog.search_closed` | Search closed · showing DistroWatch six-month popularity |
| `status.catalog.popularity` | Showing DistroWatch six-month popularity |
| `status.catalog.ready` | DistroWatch catalog ready • rankings indicate interest, not quality |
| `status.catalog.loading_popularity` | Loading DistroWatch six-month popularity… |
| `status.catalog.searching_directory` | Searching DistroWatch's full distribution directory… |
| `status.catalog.loading` | Loading distributions… |
| `status.catalog.loading_base` | Searching DistroWatch for active {base}-based distributions… |
| `status.catalog.showing_base` (plural) | Showing {count} active {base}-based distributions from DistroWatch |
| `status.catalog.loading_releases` | Resolving current {name} ISO files… |
| `status.catalog.profile_ready_no_iso` | Profile ready · no direct ISO found |
| `status.catalog.profile_ready_errors` (plural) | Profile ready · no direct ISO found · {count} source errors |
| `status.catalog.releases_loaded` (plural) | {count} direct ISO releases · {source} |
| `status.catalog.source_warnings` (plural) | {count} source warnings |
| `status.catalog.with_warnings` | {summary} · {warnings} |
| `status.catalog.omarchy` | Omarchy family · ISO releases are writable; installer-only derivatives are clearly marked |
| `status.catalog.omarchy_missing` | Omarchy is missing from the current DistroWatch directory |
| `status.catalog.windows_tools` | Windows media tools · choose a Windows ISO to unlock setup options |
| `status.catalog.windows_uses_iso` | Windows tools use the selected local ISO |
| `status.catalog.pi_selected` | Raspberry Pi image discovery selected |
| `status.catalog.pi_loading` | Loading the official Raspberry Pi Imager catalog… |
| `status.catalog.pi_compatible` | Showing images compatible with {board} |
| `status.catalog.pi_all` | Showing every Raspberry Pi image |
| `status.catalog.pi_choose` | Choose a Raspberry Pi image to download |
| `status.catalog.pi_selected_verify` | Raspberry Pi image selected • download will be extracted and verified |
| `status.catalog.iso_selected_checksum` | ISO selected • publisher checksum will be verified before use |
| `status.catalog.iso_selected_https` | ISO selected • publisher checksum unavailable; HTTPS length and boot structure will be checked |
| `status.catalog.choose_distribution` | Choose a distribution first |
| `status.catalog.choose_release` | Choose an ISO release to download |
| `status.catalog.browser_opened` | Opened the DistroWatch distribution page in your browser |
| `status.catalog.artwork_error` | Could not decode catalog artwork: {error} |

#### Downloads panel

| Key | English |
| --- | --- |
| `downloads.subtitle` | Persistent history · interrupted transfers can resume |
| `downloads.empty` | No managed downloads yet · choose an image from Discover to begin |
| `downloads.selected` | Selected download |
| `downloads.interrupted_note` | Interrupted transfers retain only owned partial files; explicit cancellation removes them. |
| `downloads.action.retry_resume` | Retry / resume |
| `downloads.action.use_image` | Use image |
| `downloads.action.remove` | Remove entry |
| `downloads.action.cancel` | Cancel download |
| `downloads.jobs_in_history` (plural) | {count} download jobs in history |
| `downloads.status.queued` | Queued |
| `downloads.status.running` | Downloading |
| `downloads.status.paused` | Paused |
| `downloads.status.interrupted` | Interrupted |
| `downloads.status.completed` | Completed |
| `downloads.status.failed` | Failed |
| `downloads.status.cancelled` | Cancelled |
| `downloads.kind.iso` | ISO |
| `downloads.kind.raspberry_pi` | Raspberry Pi image |

#### Download status lines

| Key | English |
| --- | --- |
| `status.download.history_unavailable` | Download history unavailable · {error} |
| `status.download.queued` | Download queued · it will start when the active job finishes |
| `status.download.retrying` | Retrying download · preserved bytes will be resumed when supported |
| `status.download.starting` | Starting managed download… |
| `status.download.retry_queued` | Retry queued · it will start after the active download |
| `status.download.choose_job` | Choose a download job first |
| `status.download.job_gone` | Download job no longer exists |
| `status.download.start_failed` | Could not start queued download · {error} |
| `status.download.using_completed` | Using completed download {path} |
| `status.download.completed_unavailable` | Downloaded image is unavailable · {error} |
| `status.download.history_removed` | History entry removed · completed image kept |
| `status.download.iso_cancelled` | ISO download cancelled |
| `status.download.pi_cancelled` | Raspberry Pi image download cancelled |
| `status.download.ready` | Ready · downloaded, verified, and inspected {name} · discovery remains open |
| `status.download.cancelled_cleaned` | Download cancelled • temporary data cleaned up |
| `status.download.stopped` | Download stopped · {error} |
| `status.download.paused` | Download paused • resume or cancel when ready |
| `status.download.resumed` | Download resumed |
| `status.download.cancelling` | Cancelling download safely • cleaning temporary data… |

#### Image, drive and options status lines

| Key | English |
| --- | --- |
| `status.startup` | {media} · choose an image to begin |
| `status.image.busy` | Image inspection is already running |
| `status.image.cancelled` | Image selection cancelled |
| `status.image.inspecting` | Inspecting image • compressed sources are measured after expansion… |
| `status.image.recognized` | Recognized {kind} |
| `status.image.stopped` | Image inspection stopped unexpectedly |
| `status.image.folder` | Image browser folder: {path} |
| `status.image.folder_cancelled` | Folder selection cancelled |
| `status.image.choose_first` | Choose an image first |
| `status.image.no_recent` | No recent image in that position |
| `status.prefs.save_failed` | Preferences were not saved: {error} |
| `status.target.none_eligible` | No eligible removable drive is available |
| `status.target.selected` | Target selected · confirm the physical drive before reviewing the erase plan |
| `status.target.blocked` | That drive is blocked and cannot be selected |
| `status.target.choose_first` | Choose a target drive first |
| `status.drives.refresh_paused` | Drive refresh is paused while writing • do not unplug the target |
| `status.drives.up_to_date` | Drive list is up to date • automatic detection is on |
| `status.drives.changed` | Drive details changed • list updated automatically |
| `status.drives.added` (plural) | Detected {count} new drives • list updated automatically |
| `status.drives.removed` (plural) | Removed {count} drives • list updated automatically |
| `status.drives.added_removed` | Drive list changed: {added} added, {removed} removed • updated automatically |
| `status.options.expanded` | Advanced options expanded • every choice is included in the reviewed plan |
| `status.options.collapsed` | Advanced options collapsed • configured values remain active |
| `status.options.open_needs_image` | Choose or download an image before opening media options |
| `status.checksum.algorithm` | Checksum algorithm: {algorithm} |
| `status.checksum.choose_image` | Choose an image before computing its checksum |
| `status.badblocks.off` | Destructive bad-block check disabled |
| `status.badblocks.passes` (plural) | Bad-block check: {count} destructive patterns before writing |
| `status.backup.choose_drive` | Choose a removable drive to back up |
| `status.backup.cancelled` | Drive backup cancelled |
| `status.backup.running` | Backing up {drive}… |
| `status.backup.done` | Drive image saved to {path} |
| `status.backup.failed` | {error} • last step: {step} |

#### Catalog load failures and origins (shown inside catalog.state.* lines)

| Key | English |
| --- | --- |
| `catalog.failure.network` | network unavailable |
| `catalog.failure.refresh` | refresh unavailable |
| `catalog.failure.cache` | cache unavailable |
| `catalog.failure.unsupported` | catalog response unsupported |
| `catalog.failure.service` | service unavailable |
| `catalog.source.network` | updated now |
| `catalog.source.cache` | cached |
| `catalog.source.stale_cache` | cached · refresh failed |

#### Write completion status lines (core-owned write session)

| Key | English |
| --- | --- |
| `status.write.complete` | Complete • image written and verified • target can be safely removed |
| `status.write.auth_denied` | Write cancelled before erasure • administrator authentication was cancelled or denied |
| `status.write.stopped` | Write stopped safely • media is incomplete and must be rewritten before use |
| `status.write.failed` | Write failed • {error} |

#### Discover: result counts ({source} is the catalog origin label)

| Key | English |
| --- | --- |
| `status.catalog.distributions_loaded` (plural) | {count} distributions · {source} |
| `status.catalog.pi_images_loaded` (plural) | {count} Raspberry Pi images · {source} |
| `status.catalog.base_loaded` (plural) | {count} active {base}-based distributions · {source} |


### Adapter-local strings (deliberately not in the catalog)

Strings that exist in only one interface, are input legends, or are not prose.
Each adapter keeps these; if one is shown to people it should still be
localized in that adapter.

*TUI only*

* Keyboard and mouse legends: the footer (`Tab / Shift+Tab focus · Enter select
  · ? help · q quit` and its wide variant), `Press Esc, ? or click to close`,
  ` Space/click to acknowledge `, `RECENT IMAGES · press 1-4` (the `· press 1-4`
  part), the downloads panel title legend (`persistent history · ↑/↓ select · m
  closes`), `Type to search · results update live · Esc leaves search`,
  `Omarchy quick access · press Enter to resolve ISOs`, `Windows media tools ·
  press o to choose a Windows ISO`, `Press o, Enter, or click Choose Windows ISO
  to unlock setup customizations`, `Writing is active • press x to stop safely;
  do not unplug the target`, `Download paused • press p to resume or x to
  cancel`, the `[b]` suffix on the open-page button, `Boot firmware (f): …`.
* `Start with --image /path/to/image.iso to create a plan` (mentions a CLI flag),
  the `Resize to at least 44 × 22` / `q Quit` / `Terminal too small` screen.
* Panel titles that describe TUI layout: ` Windows media features `, ` Complete
  Rufus 4.15 Windows coverage ` and its coverage lines, ` Image details ·
  official Imager feed `, ` Distribution profile · artwork `, ` Raspberry Pi
  board `, ` Compatible boot images `, the Raspberry Pi `Category / Archive /
  SHA-256 available|not listed` detail lines, and the compact
  three-bullet consequence list.
* Checksum result line `{algorithm}: {hex}` (all verbatim).

*GUI only*

* `BOOTABLE` / `v{version}` brand text and window chrome.
* The Omarchy MX Mac card (title, installer explanation, repository line,
  `Not writable to USB` badge).
* The Windows feature inventory lists (`Standard Windows installation`, … , 18
  lines), `Complete Rufus 4.15 Windows inventory · …`, `Rufus Windows features
  still being implemented`, `Choose an inspected Windows installer ISO to reveal
  the independent Windows setup checkboxes. No Windows option is applied
  silently.` These duplicate Rufus feature names and need a product decision
  about whether the TUI should show the same inventory before they are
  extracted.
* `Cancelling download safely • close again after temporary data is cleaned up`
  and `Stopping write safely • close again after completed writes are flushed`
  (window-close guards).
* The `User` fallback for the suggested local account name; file-dialog
  extension lists and the default `bootable-backup.img` name.

*Both, but not prose*

* Technical values: file extensions, `GPT`/`MBR`/`UEFI`/`BIOS + UEFI (CSM)`,
  checksum algorithm names, distribution and board names, `Omarchy`, quick
  access names `Arch` / `Debian` / `Omarchy` / `Windows` / `Raspberry Pi`
  (`All` is `discover.quick.all`).
* `ReviewedWriteSession::begin` errors and the initial
  `Waiting for administrator authentication…` progress message (core).

### CLI output (`bootable` subcommands, TUI binary)

CLI output is partly machine-oriented. Decide per line whether it is prose
(translate) or a field name scripts rely on (keep stable and English, or make
output locale-independent with a `--json`-style mode). Not changed by this
work. Candidate keys:

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
| `device_flags` | `removable`, `READ-ONLY`, `SYSTEM—BLOCKED`, `internal—blocked` (TUI `devices` listing) |

### Core strings not yet converted

These originate in `bootable-core` and still produce English. They are the next
conversions to make there (add `Message` variants, then adapters pick them up
for free):

* `Progress.message` text throughout `lib.rs`, `download.rs`, `catalog.rs`,
  `pi_catalog.rs`, `write_session.rs`, `platform/*` (for example `Stage 5/5 ·
  Inspecting boot structure and media strategy`, `Ready · downloaded ...`,
  `Waiting for administrator authentication, then revalidating the target`).
  Because `Progress` carries a `String`, either add a typed `ProgressMessage`
  alongside it or keep the string and add a `kind` the adapter can localize.
* `ReviewedWriteSession::begin` errors (`Review the write plan before
  writing`, `the reviewed write cannot be started again`) and
  `apply_progress` (`{phase} • {message}`).
* `WritePlan.steps[*].title` in `plan.rs` (for example `Unmount target
  filesystems`). Plan steps need to be generated per locale or carried as a
  `Message` plus arguments.
* `Error` `Display` text in `error.rs` (`thiserror` messages). Keep the English
  `Display` for logs and add `Error::message(locale)`.
* `ImageKind`, `WriteStrategy` `Display` impls in `model.rs`;
  `Progress::metrics` (`remaining`, `elapsed`).
* The `signature_note` reasons inside `IntegrityState::ChecksumVerified`
  (`the publisher does not publish a signature`, ...) are English strings
  stored in data; make them a typed enum before localizing them. The phrasing
  around the note is already translated.
* `DownloadJob.message` and `DownloadJob.error` (row detail text). The row
  status and kind labels are converted (`DownloadStatus::label_in`,
  `DownloadKind::label_in`).

## Open questions for review

* Is a hand-rolled catalog acceptable long term, or should the format become
  real Fluent (`.ftl`) so translators can use standard tooling (Weblate,
  Pontoon)? The key names and placeholder syntax were chosen to make that
  mechanical.
* Hindi: ship a reviewed translation or keep it listed-but-hidden?
* Should the confirmation-phrase prompt text be localized while the phrase
  stays literal? (Recommended: yes. Neither the GUI nor the TUI types the
  phrase today, so no catalog message carries it; the CLI prompt would add a
  `cli.*` message that surrounds the literal phrase.)
* Should the TUI show the same Windows feature inventory as the GUI (and the
  GUI the TUI's coverage lines)? They are currently different, listed under
  "Adapter-local strings".
