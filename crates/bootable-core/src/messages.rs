//! The shared, typed message catalog.
//!
//! Every user-facing string that core produces is a [`Message`] variant whose
//! text lives in `locales/<tag>.lang`, embedded at compile time. English is the
//! source of truth: a locale (or a single message) without a translation falls
//! back to English, and tests fail if a locale defines a key English lacks or
//! changes a message's placeholders.
//!
//! The catalog format is deliberately tiny (see `locales/en.lang`): no
//! dependencies, no runtime file access, and `&'static str` results.

use std::collections::{BTreeSet, HashMap};
use std::fmt::{self, Display, Write as _};
use std::sync::OnceLock;

use crate::locale::{Locale, PluralCategory};

macro_rules! messages {
    ($($variant:ident => $key:literal,)+) => {
        /// A user-facing message. The variant is the compile-time handle; the
        /// text comes from the locale tables.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Message {
            $($variant,)+
        }

        impl Message {
            /// Every message, in declaration order.
            pub const ALL: &'static [Message] = &[$(Message::$variant,)+];

            /// The catalog key (plural messages store `<key>.<category>`).
            pub const fn key(self) -> &'static str {
                match self {
                    $(Message::$variant => $key,)+
                }
            }
        }
    };
}

messages! {
    HelpIntro => "help.intro",
    HelpSectionSource => "help.section.source",
    HelpSectionTarget => "help.section.target",
    HelpSectionReview => "help.section.review",
    HelpSectionGeneral => "help.section.general",
    HelpSourceChooseAction => "help.source.choose.action",
    HelpSourceChooseDetail => "help.source.choose.detail",
    HelpSourceRecentAction => "help.source.recent.action",
    HelpSourceRecentDetail => "help.source.recent.detail",
    HelpSourceRecentDesktop => "help.source.recent.desktop",
    HelpSourceDiscoverAction => "help.source.discover.action",
    HelpSourceDiscoverDetail => "help.source.discover.detail",
    HelpSourceDownloadsAction => "help.source.downloads.action",
    HelpSourceDownloadsDetail => "help.source.downloads.detail",
    HelpSourceDownloadsDesktop => "help.source.downloads.desktop",
    HelpTargetRefreshAction => "help.target.refresh.action",
    HelpTargetRefreshDetail => "help.target.refresh.detail",
    HelpTargetChooseAction => "help.target.choose.action",
    HelpTargetChooseDetail => "help.target.choose.detail",
    HelpTargetChooseDesktop => "help.target.choose.desktop",
    HelpReviewPlanAction => "help.review.plan.action",
    HelpReviewPlanDetail => "help.review.plan.detail",
    HelpReviewStopAction => "help.review.stop.action",
    HelpReviewStopDetail => "help.review.stop.detail",
    HelpReviewStopDesktop => "help.review.stop.desktop",
    HelpReviewFirmwareAction => "help.review.firmware.action",
    HelpReviewFirmwareDetail => "help.review.firmware.detail",
    HelpReviewFirmwareDesktop => "help.review.firmware.desktop",
    HelpGeneralToggleAction => "help.general.toggle.action",
    HelpGeneralToggleDetail => "help.general.toggle.detail",
    HelpGeneralCloseAction => "help.general.close.action",
    HelpGeneralCloseDetail => "help.general.close.detail",
    HelpGeneralLanguageAction => "help.general.language.action",
    HelpGeneralLanguageDetail => "help.general.language.detail",
    HelpGeneralLanguageDesktop => "help.general.language.desktop",
    LanguageLabel => "language.label",
    LanguageSystemDefault => "language.system_default",
    DetailDrive => "detail.drive",
    DetailPath => "detail.path",
    DetailCapacity => "detail.capacity",
    DetailConnection => "detail.connection",
    DetailSerial => "detail.serial",
    DetailStatus => "detail.status",
    DetailMounted => "detail.mounted",
    DetailConnectionUnknown => "detail.connection.unknown",
    DetailSerialNotReported => "detail.serial.not_reported",
    DetailMountedNone => "detail.mounted.none",
    DetailMountedList => "detail.mounted.list",
    DetailMountedListMore => "detail.mounted.list_more",
    TargetSystemBlocked => "target.system_blocked",
    TargetInternalBlocked => "target.internal_blocked",
    TargetReadOnlyBlocked => "target.read_only_blocked",
    TargetEligible => "target.eligible",
    MediaNone => "media.none",
    MediaReady => "media.ready",
    MediaNoneEligible => "media.none_eligible",
    MediaSomeReady => "media.some_ready",
    ReadinessNeedsImageAction => "readiness.needs_image.action",
    ReadinessNeedsTargetAction => "readiness.needs_target.action",
    ReadinessReadyAction => "readiness.ready.action",
    ReadinessNeedsImageGuidance => "readiness.needs_image.guidance",
    ReadinessNeedsTargetGuidance => "readiness.needs_target.guidance",
    ReadinessReadyGuidance => "readiness.ready.guidance",
    WorkspaceNeedsImageStatus => "workspace.needs_image.status",
    WorkspaceNeedsTargetStatus => "workspace.needs_target.status",
    WorkspaceReadyStatus => "workspace.ready.status",
    StepSource => "step.source",
    StepTarget => "step.target",
    StepReview => "step.review",
    PhasePreparing => "phase.preparing",
    PhaseDownloading => "phase.downloading",
    PhaseReading => "phase.reading",
    PhaseWriting => "phase.writing",
    PhaseSyncing => "phase.syncing",
    PhaseVerifying => "phase.verifying",
    PhaseFinished => "phase.finished",
    IntegrityTransferChecked => "integrity.transfer_checked",
    IntegrityChecksumVerified => "integrity.checksum_verified",
    IntegrityChecksumUnsigned => "integrity.checksum_unsigned",
    IntegritySignatureVerified => "integrity.signature_verified",
    IntegrityCompletion => "integrity.completion",
    IntegrityReady => "integrity.ready",
    IntegrityFinalizedTransfer => "integrity.finalized.transfer",
    IntegrityFinalizedChecksum => "integrity.finalized.checksum",
    IntegrityFinalizedChecksumUnsigned => "integrity.finalized.checksum_unsigned",
    IntegrityFinalizedSignature => "integrity.finalized.signature",
    HeaderTagline => "header.tagline",
    HeaderTitleCreate => "header.title.create",
    HeaderSubtitleCreate => "header.subtitle.create",
    HeaderSubtitleReview => "header.subtitle.review",
    GuideTitle => "guide.title",
    CommonLabeled => "common.labeled",
    ActionDownloads => "action.downloads",
    ActionDownloadsCount => "action.downloads_count",
    ActionDownloadsCompact => "action.downloads_compact",
    ActionDiscover => "action.discover",
    ActionDiscoverCompact => "action.discover_compact",
    ActionCatalogClose => "action.catalog_close",
    ActionCatalogCloseCompact => "action.catalog_close_compact",
    ActionSetupOptions => "action.setup_options",
    ActionSetupOptionsCompact => "action.setup_options_compact",
    ActionHideOptions => "action.hide_options",
    ActionHideOptionsCompact => "action.hide_options_compact",
    ActionRefreshDrives => "action.refresh_drives",
    ActionRefresh => "action.refresh",
    ActionRetry => "action.retry",
    ActionBrowse => "action.browse",
    ActionChange => "action.change",
    ActionInspecting => "action.inspecting",
    ActionSelect => "action.select",
    ActionSelected => "action.selected",
    ActionBlocked => "action.blocked",
    ActionClose => "action.close",
    ActionCancel => "action.cancel",
    ActionCancelling => "action.cancelling",
    ActionBack => "action.back",
    ActionPause => "action.pause",
    ActionResume => "action.resume",
    ActionStop => "action.stop",
    ActionReview => "action.review",
    ActionQuit => "action.quit",
    TooltipGuide => "tooltip.guide",
    TooltipRefreshDrives => "tooltip.refresh_drives",
    TooltipRefreshDistrowatch => "tooltip.refresh_distrowatch",
    TooltipRefreshPi => "tooltip.refresh_pi",
    SourceTitle => "source.title",
    SourceHint => "source.hint",
    SourceFormats => "source.formats",
    SourceInspected => "source.inspected",
    SourceRecentTitle => "source.recent.title",
    SourceRecentEmpty => "source.recent.empty",
    SourceRecentInUse => "source.recent.in_use",
    SourceDialogTitle => "source.dialog.title",
    SourceDialogFilterIso => "source.dialog.filter_iso",
    SourceDialogFilterBackup => "source.dialog.filter_backup",
    TargetTitle => "target.title",
    TargetEmpty => "target.empty",
    TargetSelectedDrive => "target.selected_drive",
    TargetConfirmPhysical => "target.confirm_physical",
    FocusSource => "focus.source",
    FocusTarget => "focus.target",
    FocusSetup => "focus.setup",
    FocusReview => "focus.review",
    FocusDiscover => "focus.discover",
    FocusRefresh => "focus.refresh",
    OptionsWindowsTitle => "options.windows.title",
    OptionsLinuxTitle => "options.linux.title",
    OptionsSelectedCount => "options.selected_count",
    OptionsSummaryVerification => "options.summary.verification",
    OptionsWindowsPartitionScheme => "options.windows.partition_scheme",
    OptionsWindowsSchemeValue => "options.windows.scheme_value",
    OptionsWindowsBootFirmware => "options.windows.boot_firmware",
    OptionsWindowsBootFirmwareExperimental => "options.windows.boot_firmware_experimental",
    OptionsWindowsBootFirmwareValue => "options.windows.boot_firmware_value",
    OptionsWindowsBootFirmwareValueExperimental => "options.windows.boot_firmware_value_experimental",
    OptionsWindowsBootFirmwareValueCompact => "options.windows.boot_firmware_value_compact",
    OptionsWindowsBootFirmwareHint => "options.windows.boot_firmware_hint",
    OptionsWindowsBypassHardwareLabel => "options.windows.bypass_hardware.label",
    OptionsWindowsBypassHardwareShort => "options.windows.bypass_hardware.short",
    OptionsWindowsBypassHardwareOn => "options.windows.bypass_hardware.on",
    OptionsWindowsBypassHardwareOff => "options.windows.bypass_hardware.off",
    OptionsWindowsOfflineAccountLabel => "options.windows.offline_account.label",
    OptionsWindowsOfflineAccountShort => "options.windows.offline_account.short",
    OptionsWindowsOfflineAccountOn => "options.windows.offline_account.on",
    OptionsWindowsOfflineAccountOff => "options.windows.offline_account.off",
    OptionsWindowsNamedAccountLabel => "options.windows.named_account.label",
    OptionsWindowsNamedAccountShort => "options.windows.named_account.short",
    OptionsWindowsNamedAccountOn => "options.windows.named_account.on",
    OptionsWindowsNamedAccountOff => "options.windows.named_account.off",
    OptionsWindowsHostRegionLabel => "options.windows.host_region.label",
    OptionsWindowsHostRegionShort => "options.windows.host_region.short",
    OptionsWindowsHostRegionOn => "options.windows.host_region.on",
    OptionsWindowsHostRegionOff => "options.windows.host_region.off",
    OptionsWindowsPrivacyLabel => "options.windows.privacy.label",
    OptionsWindowsPrivacyShort => "options.windows.privacy.short",
    OptionsWindowsPrivacyOn => "options.windows.privacy.on",
    OptionsWindowsPrivacyOff => "options.windows.privacy.off",
    OptionsWindowsBitlockerLabel => "options.windows.bitlocker.label",
    OptionsWindowsBitlockerShort => "options.windows.bitlocker.short",
    OptionsWindowsBitlockerOn => "options.windows.bitlocker.on",
    OptionsWindowsBitlockerOff => "options.windows.bitlocker.off",
    OptionsWindowsQolLabel => "options.windows.qol.label",
    OptionsWindowsQolShort => "options.windows.qol.short",
    OptionsWindowsQolOn => "options.windows.qol.on",
    OptionsWindowsQolOff => "options.windows.qol.off",
    OptionsWindowsCa2023Label => "options.windows.ca2023.label",
    OptionsWindowsCa2023Short => "options.windows.ca2023.short",
    OptionsWindowsCa2023On => "options.windows.ca2023.on",
    OptionsWindowsCa2023Off => "options.windows.ca2023.off",
    OptionsWindowsSkusipolicyLabel => "options.windows.skusipolicy.label",
    OptionsWindowsSkusipolicyShort => "options.windows.skusipolicy.short",
    OptionsWindowsSkusipolicyOn => "options.windows.skusipolicy.on",
    OptionsWindowsSkusipolicyOff => "options.windows.skusipolicy.off",
    OptionsWindowsSmodeLabel => "options.windows.smode.label",
    OptionsWindowsSmodeShort => "options.windows.smode.short",
    OptionsWindowsSmodeOn => "options.windows.smode.on",
    OptionsWindowsSmodeOff => "options.windows.smode.off",
    StatusWindowsChooseInstaller => "status.windows.choose_installer",
    StatusWindowsNotWindows => "status.windows.not_windows",
    StatusWindowsScheme => "status.windows.scheme",
    StatusWindowsFirmware => "status.windows.firmware",
    OptionsWindowsHeading => "options.windows.heading",
    OptionsWindowsInstallerReady => "options.windows.installer_ready",
    OptionsWindowsInstallerLocked => "options.windows.installer_locked",
    OptionsWindowsChooseIso => "options.windows.choose_iso",
    OptionsWindowsReplaceIso => "options.windows.replace_iso",
    OptionsWindowsUnavailableNote => "options.windows.unavailable_note",
    OptionsWindowsSilentInstallWarning => "options.windows.silent_install_warning",
    OptionsLinuxLayout => "options.linux.layout",
    OptionsLinuxLayoutShort => "options.linux.layout_short",
    OptionsLinuxVerify => "options.linux.verify",
    OptionsLinuxVerifyShort => "options.linux.verify_short",
    OptionsLinuxBootRecordsShort => "options.linux.boot_records_short",
    OptionsLinuxUnmountShort => "options.linux.unmount_short",
    OptionsToolsTitle => "options.tools.title",
    OptionsToolsSubtitle => "options.tools.subtitle",
    OptionsToolsBadBlocksOff => "options.tools.bad_blocks_off",
    OptionsToolsBadBlocksPasses => "options.tools.bad_blocks_passes",
    OptionsToolsVerifyImage => "options.tools.verify_image",
    OptionsToolsImageFolder => "options.tools.image_folder",
    OptionsToolsBackupDrive => "options.tools.backup_drive",
}

/// A named value substituted for `{name}` in a message.
pub type Arg<'a> = (&'a str, &'a dyn Display);

impl Message {
    /// The message with its placeholders left as written, for messages that
    /// have none. Plural messages resolve to their `other` form.
    pub fn text(self, locale: Locale) -> &'static str {
        resolve(locale, self.key(), None).unwrap_or_else(|| self.key())
    }

    /// The message with `{name}` placeholders replaced from `args`.
    /// Unknown placeholders are left visible rather than dropped.
    pub fn format(self, locale: Locale, args: &[Arg<'_>]) -> String {
        interpolate(self.text(locale), args)
    }

    /// A plural message for `count`, which is also available as `{count}`.
    pub fn plural(self, locale: Locale, count: u64, args: &[Arg<'_>]) -> String {
        let category = locale.plural_category(count);
        let template = resolve(locale, self.key(), Some(category)).unwrap_or_else(|| self.key());
        let mut all: Vec<Arg<'_>> = Vec::with_capacity(args.len() + 1);
        all.push(("count", &count));
        all.extend_from_slice(args);
        interpolate(template, &all)
    }

    /// True when the English source defines plural forms for this message.
    pub fn is_plural(self) -> bool {
        table(Locale::SOURCE)
            .entries
            .contains_key(format!("{}.other", self.key()).as_str())
    }
}

/// A [`Locale`] bound to the catalog, so adapters write `t.text(Message::X)`
/// instead of threading the locale through every call.
///
/// It is `Copy` and holds nothing but the locale; keep it in app state and
/// rebuild it (`locale.strings()`) when the user changes the language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strings {
    locale: Locale,
}

impl Strings {
    pub const fn new(locale: Locale) -> Self {
        Self { locale }
    }

    pub const fn locale(self) -> Locale {
        self.locale
    }

    /// [`Message::text`] in this locale.
    pub fn text(self, message: Message) -> &'static str {
        message.text(self.locale)
    }

    /// [`Message::format`] in this locale.
    pub fn format(self, message: Message, args: &[Arg<'_>]) -> String {
        message.format(self.locale, args)
    }

    /// [`Message::plural`] in this locale.
    pub fn plural(self, message: Message, count: u64, args: &[Arg<'_>]) -> String {
        message.plural(self.locale, count, args)
    }

    /// The message upper-cased for section headings and badges (`SOURCE`,
    /// `ERASES DATA`). Catalog text is stored in sentence case; scripts
    /// without case (CJK) are unchanged.
    pub fn heading(self, message: Message) -> String {
        message.text(self.locale).to_uppercase()
    }
}

impl Locale {
    /// This locale bound to the message catalog.
    pub const fn strings(self) -> Strings {
        Strings::new(self)
    }
}

impl fmt::Display for Message {
    /// English text, for logs and diagnostics. UI code should use [`Message::text`].
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.text(Locale::SOURCE))
    }
}

/// How completely a locale covers the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub translated: usize,
    pub total: usize,
    /// Keys that currently fall back to English.
    pub missing: Vec<&'static str>,
}

impl Locale {
    /// Which messages this locale translates; the rest fall back to English.
    pub fn coverage(self) -> Coverage {
        let entries = &table(self).entries;
        let missing = Message::ALL
            .iter()
            .map(|message| message.key())
            .filter(|key| {
                !entries.contains_key(*key) && !entries.contains_key(plural_key(key).as_str())
            })
            .collect::<Vec<_>>();
        Coverage {
            translated: Message::ALL.len() - missing.len(),
            total: Message::ALL.len(),
            missing,
        }
    }

    /// Locales a language picker should offer: English plus every locale with
    /// at least one translated message.
    pub fn available() -> Vec<Locale> {
        Locale::ALL
            .iter()
            .copied()
            .filter(|locale| locale.is_source() || locale.coverage().translated > 0)
            .collect()
    }
}

fn plural_key(key: &str) -> String {
    format!("{key}.other")
}

fn resolve(locale: Locale, key: &str, category: Option<PluralCategory>) -> Option<&'static str> {
    [locale, Locale::SOURCE]
        .into_iter()
        .find_map(|candidate| lookup(candidate, key, category))
}

fn lookup(locale: Locale, key: &str, category: Option<PluralCategory>) -> Option<&'static str> {
    let entries = &table(locale).entries;
    let get = |key: String| entries.get(key.as_str()).map(String::as_str);
    category
        .and_then(|category| get(format!("{key}.{}", category.suffix())))
        .or_else(|| get(format!("{key}.other")))
        .or_else(|| get(key.to_owned()))
}

fn interpolate(template: &str, args: &[Arg<'_>]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                output.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                output.push('}');
            }
            '{' => {
                let mut name = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    name.push(next);
                }
                match args.iter().find(|(candidate, _)| *candidate == name) {
                    Some((_, value)) if closed => {
                        let _ = write!(output, "{value}");
                    }
                    _ => {
                        output.push('{');
                        output.push_str(&name);
                        if closed {
                            output.push('}');
                        }
                    }
                }
            }
            other => output.push(other),
        }
    }
    output
}

// ---- Locale tables --------------------------------------------------------

struct Table {
    entries: HashMap<&'static str, String>,
    /// Malformed lines that were skipped; asserted empty by the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    errors: Vec<String>,
}

const fn source(locale: Locale) -> &'static str {
    match locale {
        Locale::En => include_str!("../locales/en.lang"),
        Locale::Es => include_str!("../locales/es.lang"),
        Locale::Fr => include_str!("../locales/fr.lang"),
        Locale::De => include_str!("../locales/de.lang"),
        Locale::PtBr => include_str!("../locales/pt-BR.lang"),
        Locale::ZhHans => include_str!("../locales/zh-Hans.lang"),
        Locale::Ja => include_str!("../locales/ja.lang"),
        Locale::Ru => include_str!("../locales/ru.lang"),
        Locale::Hi => include_str!("../locales/hi.lang"),
    }
}

fn table(locale: Locale) -> &'static Table {
    static TABLES: [OnceLock<Table>; Locale::ALL.len()] =
        [const { OnceLock::new() }; Locale::ALL.len()];
    TABLES[locale as usize].get_or_init(|| parse(source(locale)))
}

/// Parses a catalog. Malformed lines are skipped and reported in `errors`, so a
/// bad line can never take the rest of a locale down; tests require no errors.
fn parse(source: &'static str) -> Table {
    let mut entries = HashMap::new();
    let mut errors = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            errors.push(format!("line {number}: expected `key = value`"));
            continue;
        };
        let key = key.trim();
        let valid_key = !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_');
        if !valid_key {
            errors.push(format!("line {number}: invalid key `{key}`"));
            continue;
        }
        let value = match unescape(value.trim()) {
            Ok(value) if !value.is_empty() => value,
            Ok(_) => {
                errors.push(format!("line {number}: `{key}` has an empty value"));
                continue;
            }
            Err(error) => {
                errors.push(format!("line {number}: `{key}`: {error}"));
                continue;
            }
        };
        if let Err(error) = placeholders(&value) {
            errors.push(format!("line {number}: `{key}`: {error}"));
            continue;
        }
        if entries.insert(key, value).is_some() {
            errors.push(format!("line {number}: duplicate key `{key}`"));
        }
    }
    Table { entries, errors }
}

fn unescape(value: &str) -> Result<String, String> {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('n') => output.push('\n'),
            Some('\\') => output.push('\\'),
            other => return Err(format!("unknown escape `\\{}`", other.unwrap_or(' '))),
        }
    }
    Ok(output)
}

/// The set of `{name}` placeholders in a template, or why it is malformed.
fn placeholders(template: &str) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    let mut chars = template.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
            }
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) if c.is_ascii_lowercase() || c == '_' => name.push(c),
                        _ => return Err("malformed placeholder".into()),
                    }
                }
                if name.is_empty() {
                    return Err("empty placeholder".into());
                }
                names.insert(name);
            }
            '}' => return Err("unbalanced `}`".into()),
            _ => {}
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Base key of a table key and its plural suffix, if it has one.
    fn split_plural(key: &str) -> (&str, Option<&str>) {
        match key.rsplit_once('.') {
            Some((base, suffix @ ("one" | "few" | "many" | "other"))) => (base, Some(suffix)),
            _ => (key, None),
        }
    }

    fn placeholder_set(locale: Locale, key: &str) -> BTreeSet<String> {
        placeholders(&table(locale).entries[key]).expect("validated at parse time")
    }

    #[test]
    fn every_catalog_parses_cleanly() {
        for locale in Locale::ALL {
            let table = table(*locale);
            assert!(
                table.errors.is_empty(),
                "{locale}.lang has errors: {:#?}",
                table.errors
            );
        }
    }

    #[test]
    fn message_keys_are_unique_and_exactly_the_english_keys() {
        let declared = Message::ALL
            .iter()
            .map(|message| message.key())
            .collect::<Vec<_>>();
        let unique = declared.iter().collect::<BTreeSet<_>>();
        assert_eq!(declared.len(), unique.len(), "duplicate Message key");

        let english = table(Locale::En)
            .entries
            .keys()
            .map(|key| split_plural(key).0)
            .collect::<BTreeSet<_>>();
        for key in &declared {
            assert!(
                english.contains(key),
                "Message key `{key}` missing from en.lang"
            );
        }
        for key in &english {
            assert!(
                declared.contains(key),
                "en.lang key `{key}` has no Message variant"
            );
        }
    }

    #[test]
    fn plural_forms_are_all_or_nothing_and_include_other() {
        for locale in Locale::ALL {
            let entries = &table(*locale).entries;
            let mut by_base: HashMap<&str, BTreeSet<&str>> = HashMap::new();
            for key in entries.keys() {
                if let (base, Some(suffix)) = split_plural(key) {
                    by_base.entry(base).or_default().insert(suffix);
                }
            }
            for (base, forms) in by_base {
                assert!(
                    !entries.contains_key(base),
                    "{locale}: `{base}` is both plural and singular"
                );
                assert!(forms.contains("other"), "{locale}: `{base}` lacks `.other`");
                let expected = locale
                    .plural_categories()
                    .iter()
                    .map(|category| category.suffix())
                    .chain(["other"])
                    .collect::<BTreeSet<_>>();
                // English additionally keeps an `.other`-only reachable `one`.
                assert_eq!(
                    forms, expected,
                    "{locale}: `{base}` plural forms must be exactly the CLDR categories"
                );
            }
        }
    }

    #[test]
    fn locales_define_no_key_english_lacks_and_keep_placeholders_and_shape() {
        let english = &table(Locale::En).entries;
        for locale in Locale::ALL.iter().filter(|locale| !locale.is_source()) {
            for key in table(*locale).entries.keys() {
                let (base, suffix) = split_plural(key);
                // Plural keys are compared through `.other`, which every
                // locale's plural set contains.
                let english_key = if suffix.is_some() {
                    format!("{base}.other")
                } else {
                    (*key).to_owned()
                };
                assert!(
                    english.contains_key(english_key.as_str()),
                    "{locale}: key `{key}` does not exist in en.lang"
                );
                let mut expected = placeholder_set(Locale::En, &english_key);
                let mut actual = placeholder_set(*locale, key);
                if suffix.is_some() {
                    // `{count}` is implicit in every plural form.
                    expected.remove("count");
                    actual.remove("count");
                }
                assert_eq!(
                    actual, expected,
                    "{locale}: placeholders of `{key}` differ from en"
                );
            }
        }
    }

    #[test]
    fn plural_forms_in_english_agree_on_placeholders() {
        let english = &table(Locale::En).entries;
        for message in Message::ALL.iter().filter(|message| message.is_plural()) {
            let one = format!("{}.one", message.key());
            let other = format!("{}.other", message.key());
            if english.contains_key(one.as_str()) {
                let mut left = placeholder_set(Locale::En, &one);
                let mut right = placeholder_set(Locale::En, &other);
                left.remove("count");
                right.remove("count");
                assert_eq!(left, right, "{}", message.key());
            }
        }
    }

    #[test]
    fn missing_translations_are_reported_but_never_fatal() {
        for locale in Locale::ALL {
            let coverage = locale.coverage();
            assert_eq!(coverage.total, Message::ALL.len());
            assert_eq!(coverage.translated + coverage.missing.len(), coverage.total);
            if locale.is_source() {
                assert!(coverage.missing.is_empty(), "en must be complete");
            }
            eprintln!(
                "i18n coverage {:>8}: {}/{} ({} fall back to English)",
                locale.tag(),
                coverage.translated,
                coverage.total,
                coverage.missing.len()
            );
            // Fallback is exercised for every message, translated or not.
            for message in Message::ALL {
                assert!(!message.text(*locale).is_empty());
                assert_ne!(message.text(*locale), message.key(), "{locale} {message:?}");
            }
        }
    }

    #[test]
    fn available_locales_always_include_english_first() {
        let available = Locale::available();
        assert_eq!(available.first(), Some(&Locale::En));
        assert!(
            !available.contains(&Locale::Hi),
            "hi ships no translations yet"
        );
    }

    #[test]
    fn interpolation_substitutes_escapes_and_keeps_unknowns() {
        assert_eq!(
            interpolate("{a} and {b}", &[("a", &1), ("b", &"two")]),
            "1 and two"
        );
        assert_eq!(interpolate("{{literal}} {a}", &[("a", &3)]), "{literal} 3");
        assert_eq!(interpolate("{missing}", &[]), "{missing}");
        assert_eq!(
            interpolate("tail {broken", &[("broken", &1)]),
            "tail {broken"
        );
        assert!(placeholders("{Bad}").is_err());
        assert!(placeholders("{a").is_err());
        assert!(placeholders("a}").is_err());
        assert_eq!(unescape(r"a\nb\\c").as_deref(), Ok("a\nb\\c"));
        assert!(unescape(r"\q").is_err());
    }

    #[test]
    fn plurals_pick_the_locale_category_and_fall_back() {
        let ready = |locale, count| Message::MediaReady.plural(locale, count, &[]);
        assert_eq!(ready(Locale::En, 1), "1 removable drive ready");
        assert_eq!(ready(Locale::En, 0), "0 removable drives ready");
        assert_eq!(ready(Locale::En, 3), "3 removable drives ready");
        // Russian selects one/few/many.
        let one = ready(Locale::Ru, 1);
        let few = ready(Locale::Ru, 3);
        let many = ready(Locale::Ru, 5);
        assert!(one != few && few != many && one != many);
        assert!(one.starts_with('1') && few.starts_with('3') && many.starts_with('5'));
        // Untranslated locale falls back to English per message.
        assert_eq!(ready(Locale::Hi, 2), "2 removable drives ready");
        assert!(Message::MediaReady.is_plural());
        assert!(!Message::MediaNone.is_plural());
    }

    #[test]
    fn text_falls_back_to_english_for_untranslated_locales() {
        assert_eq!(
            Message::TargetEligible.text(Locale::Hi),
            "Removable · eligible"
        );
    }
}
