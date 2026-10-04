use crate::locale::Locale;
use crate::messages::Message;
use crate::model::{Device, format_bytes, target_eligibility_label_in};

/// One labelled fact about a drive, shown identically by both interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailRow {
    pub label: &'static str,
    pub value: String,
}

const MAX_LISTED_MOUNTS: usize = 3;

/// Facts a user needs to confirm that a drive is the physical one they mean.
pub fn device_details(device: &Device) -> Vec<DetailRow> {
    device_details_in(Locale::SOURCE, device)
}

/// [`device_details`] with labels and values in `locale`. Rows keep the same
/// order in every locale; only their text changes.
pub fn device_details_in(locale: Locale, device: &Device) -> Vec<DetailRow> {
    let row = |label: Message, value: String| DetailRow {
        label: label.text(locale),
        value,
    };
    vec![
        row(Message::DetailDrive, device.display_name()),
        row(Message::DetailPath, device.path.display().to_string()),
        row(Message::DetailCapacity, format_bytes(device.capacity)),
        row(
            Message::DetailConnection,
            device
                .transport
                .clone()
                .filter(|transport| !transport.is_empty())
                .unwrap_or_else(|| Message::DetailConnectionUnknown.text(locale).into()),
        ),
        row(
            Message::DetailSerial,
            masked_serial(locale, device.serial.as_deref()),
        ),
        row(
            Message::DetailStatus,
            target_eligibility_label_in(locale, device).into(),
        ),
        row(Message::DetailMounted, mounted_summary(locale, device)),
    ]
}

fn masked_serial(locale: Locale, serial: Option<&str>) -> String {
    let serial = serial.map(str::trim).filter(|serial| !serial.is_empty());
    match serial {
        None => Message::DetailSerialNotReported.text(locale).into(),
        Some(serial) => {
            let tail = serial.chars().rev().take(4).collect::<Vec<_>>();
            let tail = tail.into_iter().rev().collect::<String>();
            if serial.chars().count() > 4 {
                format!("…{tail}")
            } else {
                tail
            }
        }
    }
}

fn mounted_summary(locale: Locale, device: &Device) -> String {
    if device.mounts.is_empty() {
        return Message::DetailMountedNone.text(locale).into();
    }
    let listed = device
        .mounts
        .iter()
        .take(MAX_LISTED_MOUNTS)
        .map(|mount| mount.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let hidden = device.mounts.len().saturating_sub(MAX_LISTED_MOUNTS);
    if hidden > 0 {
        Message::DetailMountedListMore.format(locale, &[("listed", &listed), ("hidden", &hidden)])
    } else {
        Message::DetailMountedList.format(locale, &[("listed", &listed)])
    }
}

/// A user-facing action shared by both interfaces, with each interface's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelpEntry {
    pub action: &'static str,
    pub detail: &'static str,
    pub desktop: &'static str,
    pub terminal: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelpSection {
    pub title: &'static str,
    pub entries: &'static [HelpEntry],
}

const fn entry(
    action: &'static str,
    detail: &'static str,
    desktop: &'static str,
    terminal: &'static str,
) -> HelpEntry {
    HelpEntry {
        action,
        detail,
        desktop,
        terminal,
    }
}

/// English-only guide text, kept for callers that have not adopted
/// [`help_intro`]. A test pins it to the English catalog.
pub const HELP_INTRO: &str = "Source → Target → Review & write. Nothing is written until you review the plan and acknowledge the erase.";

/// English-only guide sections; see [`help_sections`] for the localized form.
pub const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        title: "Source",
        entries: &[
            entry(
                "Choose an image",
                "Browse for an ISO, IMG, RAW, or compressed image",
                "Ctrl+O",
                "o",
            ),
            entry(
                "Use a recent image",
                "Re-inspect one of your last images",
                "Click a recent image",
                "1–4",
            ),
            entry(
                "Discover images",
                "Open or close the distribution catalog",
                "Ctrl+G",
                "g",
            ),
            entry(
                "Downloads",
                "Open or close the download manager",
                "Downloads button",
                "m",
            ),
        ],
    },
    HelpSection {
        title: "Target",
        entries: &[
            entry("Refresh drives", "Rescan removable media", "Ctrl+R", "r"),
            entry(
                "Choose a drive",
                "Selecting is always explicit; fixed disks are blocked",
                "Click a drive",
                "↑ ↓ · j k",
            ),
        ],
    },
    HelpSection {
        title: "Review & write",
        entries: &[
            entry(
                "Review plan",
                "Open the plan; nothing is written yet",
                "Ctrl+P",
                "p",
            ),
            entry(
                "Stop a write",
                "Stops at the next safe boundary",
                "Stop button",
                "x",
            ),
        ],
    },
    HelpSection {
        title: "General",
        entries: &[
            entry(
                "Show or hide this guide",
                "Reopen it any time",
                "F1 · Ctrl+/",
                "?",
            ),
            entry(
                "Close a panel",
                "Dismiss this guide or the catalog",
                "Esc",
                "Esc",
            ),
            entry(
                "Change the language",
                "Applies to both interfaces; saved for next time",
                "Language menu",
                "L",
            ),
        ],
    },
];

/// A [`HelpSection`] rendered for one locale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalizedHelpSection {
    pub title: &'static str,
    pub entries: Vec<HelpEntry>,
}

/// How a help entry names the input for one interface: a key chord that is the
/// same in every language, or a localized description such as "Stop button".
#[derive(Clone, Copy)]
enum Input {
    Keys(&'static str),
    Text(Message),
}

impl Input {
    fn render(self, locale: Locale) -> &'static str {
        match self {
            Self::Keys(keys) => keys,
            Self::Text(message) => message.text(locale),
        }
    }
}

struct EntrySpec {
    action: Message,
    detail: Message,
    desktop: Input,
    terminal: Input,
}

const fn spec(action: Message, detail: Message, desktop: Input, terminal: Input) -> EntrySpec {
    EntrySpec {
        action,
        detail,
        desktop,
        terminal,
    }
}

use Input::{Keys, Text};

const SOURCE_ENTRIES: &[EntrySpec] = &[
    spec(
        Message::HelpSourceChooseAction,
        Message::HelpSourceChooseDetail,
        Keys("Ctrl+O"),
        Keys("o"),
    ),
    spec(
        Message::HelpSourceRecentAction,
        Message::HelpSourceRecentDetail,
        Text(Message::HelpSourceRecentDesktop),
        Keys("1–4"),
    ),
    spec(
        Message::HelpSourceDiscoverAction,
        Message::HelpSourceDiscoverDetail,
        Keys("Ctrl+G"),
        Keys("g"),
    ),
    spec(
        Message::HelpSourceDownloadsAction,
        Message::HelpSourceDownloadsDetail,
        Text(Message::HelpSourceDownloadsDesktop),
        Keys("m"),
    ),
];

const TARGET_ENTRIES: &[EntrySpec] = &[
    spec(
        Message::HelpTargetRefreshAction,
        Message::HelpTargetRefreshDetail,
        Keys("Ctrl+R"),
        Keys("r"),
    ),
    spec(
        Message::HelpTargetChooseAction,
        Message::HelpTargetChooseDetail,
        Text(Message::HelpTargetChooseDesktop),
        Keys("↑ ↓ · j k"),
    ),
];

const REVIEW_ENTRIES: &[EntrySpec] = &[
    spec(
        Message::HelpReviewPlanAction,
        Message::HelpReviewPlanDetail,
        Keys("Ctrl+P"),
        Keys("p"),
    ),
    spec(
        Message::HelpReviewStopAction,
        Message::HelpReviewStopDetail,
        Text(Message::HelpReviewStopDesktop),
        Keys("x"),
    ),
];

const GENERAL_ENTRIES: &[EntrySpec] = &[
    spec(
        Message::HelpGeneralToggleAction,
        Message::HelpGeneralToggleDetail,
        Keys("F1 · Ctrl+/"),
        Keys("?"),
    ),
    spec(
        Message::HelpGeneralCloseAction,
        Message::HelpGeneralCloseDetail,
        Keys("Esc"),
        Keys("Esc"),
    ),
    spec(
        Message::HelpGeneralLanguageAction,
        Message::HelpGeneralLanguageDetail,
        Text(Message::HelpGeneralLanguageDesktop),
        Keys("L"),
    ),
];

/// The one-line workflow summary shown at the top of the guide.
pub fn help_intro(locale: Locale) -> &'static str {
    Message::HelpIntro.text(locale)
}

/// The guide, in the same section and entry order for every locale. Key
/// chords are identical across languages; only descriptions are translated.
pub fn help_sections(locale: Locale) -> Vec<LocalizedHelpSection> {
    [
        (Message::HelpSectionSource, SOURCE_ENTRIES),
        (Message::HelpSectionTarget, TARGET_ENTRIES),
        (Message::HelpSectionReview, REVIEW_ENTRIES),
        (Message::HelpSectionGeneral, GENERAL_ENTRIES),
    ]
    .into_iter()
    .map(|(title, entries)| LocalizedHelpSection {
        title: title.text(locale),
        entries: entries
            .iter()
            .map(|entry| HelpEntry {
                action: entry.action.text(locale),
                detail: entry.detail.text(locale),
                desktop: entry.desktop.render(locale),
                terminal: entry.terminal.render(locale),
            })
            .collect(),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{DeviceId, MountPoint};

    fn device() -> Device {
        Device {
            id: DeviceId::new("usb-1"),
            path: PathBuf::from("/dev/sdz"),
            vendor: Some("Acme".into()),
            model: Some("Stick".into()),
            serial: Some("ABCDEF123456".into()),
            transport: Some("usb".into()),
            capacity: 16 * 1024 * 1024 * 1024,
            removable: true,
            read_only: false,
            system_disk: false,
            mounts: Vec::new(),
        }
    }

    fn value<'a>(rows: &'a [DetailRow], label: &str) -> &'a str {
        &rows
            .iter()
            .find(|row| row.label == label)
            .expect("row")
            .value
    }

    #[test]
    fn serial_is_masked_to_its_tail() {
        let rows = device_details(&device());
        assert_eq!(value(&rows, "Serial"), "…3456");
        let mut short = device();
        short.serial = Some("12".into());
        assert_eq!(value(&device_details(&short), "Serial"), "12");
        short.serial = None;
        assert_eq!(value(&device_details(&short), "Serial"), "not reported");
    }

    #[test]
    fn mounts_are_summarized_and_capped() {
        let mut mounted = device();
        assert_eq!(
            value(&device_details(&mounted), "Mounted"),
            "nothing mounted"
        );
        mounted.mounts = (0..5)
            .map(|index| MountPoint {
                device: PathBuf::from(format!("/dev/sdz{index}")),
                path: PathBuf::from(format!("/run/media/u/v{index}")),
            })
            .collect();
        let rows = device_details(&mounted);
        let summary = value(&rows, "Mounted");
        assert!(summary.contains("/run/media/u/v0"));
        assert!(!summary.contains("/run/media/u/v3"));
        assert!(summary.contains("+2 more"));
    }

    #[test]
    fn blocked_drives_say_so() {
        let mut internal = device();
        internal.removable = false;
        assert_eq!(
            value(&device_details(&internal), "Status"),
            "Internal disk · blocked"
        );
    }

    #[test]
    fn english_help_from_the_catalog_matches_the_constants() {
        assert_eq!(help_intro(Locale::En), HELP_INTRO);
        let sections = help_sections(Locale::En);
        assert_eq!(sections.len(), HELP_SECTIONS.len());
        for (localized, constant) in sections.iter().zip(HELP_SECTIONS) {
            assert_eq!(localized.title, constant.title);
            assert_eq!(localized.entries, constant.entries);
        }
    }

    #[test]
    fn help_has_the_same_shape_in_every_locale() {
        let english = help_sections(Locale::En);
        for locale in Locale::ALL {
            let sections = help_sections(*locale);
            assert_eq!(sections.len(), english.len());
            for (section, reference) in sections.iter().zip(&english) {
                assert_eq!(section.entries.len(), reference.entries.len());
                for (entry, reference) in section.entries.iter().zip(&reference.entries) {
                    assert!(!entry.action.is_empty() && !entry.detail.is_empty());
                    assert!(!entry.desktop.is_empty() && !entry.terminal.is_empty());
                    // Terminal inputs are key chords and never translated.
                    assert_eq!(entry.terminal, reference.terminal);
                }
            }
        }
    }

    #[test]
    fn device_details_keep_their_order_and_translate_labels() {
        let mut unmounted = device();
        unmounted.serial = None;
        let english = device_details_in(Locale::En, &unmounted);
        let spanish = device_details_in(Locale::Es, &unmounted);
        assert_eq!(english.len(), spanish.len());
        assert_eq!(english[0].label, "Drive");
        assert_eq!(spanish[0].label, "Unidad");
        assert_eq!(value(&spanish, "N.º de serie"), "no informado");
        assert_eq!(value(&spanish, "Estado"), "Extraíble · apto");
        assert_eq!(
            value(&device_details_in(Locale::Fr, &unmounted), "Monté"),
            "rien n'est monté"
        );
    }

    #[test]
    fn every_help_entry_names_both_interfaces() {
        for section in HELP_SECTIONS {
            for entry in section.entries {
                assert!(!entry.desktop.is_empty() && !entry.terminal.is_empty());
            }
        }
    }
}
