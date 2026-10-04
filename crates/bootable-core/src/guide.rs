use crate::model::{Device, format_bytes, target_eligibility_label};

/// One labelled fact about a drive, shown identically by both interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailRow {
    pub label: &'static str,
    pub value: String,
}

const MAX_LISTED_MOUNTS: usize = 3;

/// Facts a user needs to confirm that a drive is the physical one they mean.
pub fn device_details(device: &Device) -> Vec<DetailRow> {
    let row = |label, value: String| DetailRow { label, value };
    let mut rows = vec![
        row("Drive", device.display_name()),
        row("Path", device.path.display().to_string()),
        row("Capacity", format_bytes(device.capacity)),
        row(
            "Connection",
            device
                .transport
                .clone()
                .filter(|transport| !transport.is_empty())
                .unwrap_or_else(|| "unknown".into()),
        ),
        row("Serial", masked_serial(device.serial.as_deref())),
        row("Status", target_eligibility_label(device).into()),
    ];
    rows.push(row("Mounted", mounted_summary(device)));
    rows
}

fn masked_serial(serial: Option<&str>) -> String {
    let serial = serial.map(str::trim).filter(|serial| !serial.is_empty());
    match serial {
        None => "not reported".into(),
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

fn mounted_summary(device: &Device) -> String {
    if device.mounts.is_empty() {
        return "nothing mounted".into();
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
        format!("{listed} +{hidden} more · unmounted before writing")
    } else {
        format!("{listed} · unmounted before writing")
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

pub const HELP_INTRO: &str = "Source → Target → Review & write. Nothing is written until you review the plan and acknowledge the erase.";

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
        ],
    },
];

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
    fn every_help_entry_names_both_interfaces() {
        for section in HELP_SECTIONS {
            for entry in section.entries {
                assert!(!entry.desktop.is_empty() && !entry.terminal.is_empty());
            }
        }
    }
}
