use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountPoint {
    pub device: PathBuf,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub path: PathBuf,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub transport: Option<String>,
    pub capacity: u64,
    pub removable: bool,
    pub read_only: bool,
    pub system_disk: bool,
    pub mounts: Vec<MountPoint>,
}

impl Device {
    pub fn display_name(&self) -> String {
        let name = [self.vendor.as_deref(), self.model.as_deref()]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if name.is_empty() {
            self.path.display().to_string()
        } else {
            name
        }
    }

    pub fn is_eligible_target(&self) -> bool {
        self.removable && !self.read_only && !self.system_disk
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowsPayload {
    Wim,
    Esd,
    SplitWim,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageCompression {
    Xz,
    Gzip,
    Zstandard,
    Bzip2,
}

impl fmt::Display for ImageCompression {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Xz => "XZ",
            Self::Gzip => "gzip",
            Self::Zstandard => "Zstandard",
            Self::Bzip2 => "bzip2",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressedImageKind {
    HybridIso,
    RawDiskImage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageKind {
    WindowsInstaller {
        payload: WindowsPayload,
        payload_size: Option<u64>,
    },
    HybridIso,
    RawDiskImage,
    CompressedDiskImage {
        compression: ImageCompression,
        inner: CompressedImageKind,
    },
    OpticalIso,
}

impl fmt::Display for ImageKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WindowsInstaller {
                payload,
                payload_size,
            } => {
                write!(formatter, "Windows installer ({payload:?}")?;
                if let Some(size) = payload_size {
                    write!(formatter, ", {} payload", format_bytes(*size))?;
                }
                formatter.write_str(")")
            }
            Self::HybridIso => formatter.write_str("hybrid bootable ISO"),
            Self::RawDiskImage => formatter.write_str("raw disk image"),
            Self::CompressedDiskImage { compression, inner } => write!(
                formatter,
                "{compression}-compressed {}",
                match inner {
                    CompressedImageKind::HybridIso => "hybrid bootable ISO",
                    CompressedImageKind::RawDiskImage => "raw disk image",
                }
            ),
            Self::OpticalIso => formatter.write_str("optical-only ISO"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageReport {
    pub path: PathBuf,
    pub size: u64,
    pub kind: ImageKind,
    pub volume_label: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewReadiness {
    NeedsImage,
    NeedsTarget,
    Ready,
}

impl ReviewReadiness {
    pub fn action_label(self) -> &'static str {
        match self {
            Self::NeedsImage => "Choose image first",
            Self::NeedsTarget => "Choose a removable drive",
            Self::Ready => "Review plan",
        }
    }

    pub fn guidance(self) -> &'static str {
        match self {
            Self::NeedsImage => "Choose or download an image to continue",
            Self::NeedsTarget => "Connect and choose a removable drive to continue",
            Self::Ready => "Ready to review the image, target, and erase plan",
        }
    }
}

pub fn review_readiness(image: Option<&ImageReport>, target: Option<&Device>) -> ReviewReadiness {
    if image.is_none() {
        ReviewReadiness::NeedsImage
    } else if !target.is_some_and(Device::is_eligible_target) {
        ReviewReadiness::NeedsTarget
    } else {
        ReviewReadiness::Ready
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteStrategy {
    RawVerified,
    WindowsFat32 {
        payload: WindowsPayload,
        split_payload: bool,
        partition_scheme: WindowsPartitionScheme,
    },
}

impl fmt::Display for WriteStrategy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RawVerified => formatter.write_str("raw write + byte verification"),
            Self::WindowsFat32 {
                split_payload: true,
                partition_scheme,
                ..
            } => write!(
                formatter,
                "{partition_scheme} + FAT32 Windows installer with split WIM"
            ),
            Self::WindowsFat32 {
                split_payload: false,
                partition_scheme,
                ..
            } => write!(formatter, "{partition_scheme} + FAT32 Windows installer"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub title: String,
    pub destructive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritePlan {
    pub image: ImageReport,
    pub target: Device,
    pub strategy: WriteStrategy,
    pub options: WriteOptions,
    pub steps: Vec<PlanStep>,
    pub required_tools: Vec<String>,
    pub confirmation_phrase: String,
}

impl WritePlan {
    pub fn confirmation_matches(&self, value: &str) -> bool {
        value == self.confirmation_phrase
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteOptions {
    pub windows: WindowsExperienceOptions,
    pub windows_partition_scheme: WindowsPartitionScheme,
    pub bad_block_check: BadBlockCheck,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowsPartitionScheme {
    #[default]
    Gpt,
    Mbr,
}

impl fmt::Display for WindowsPartitionScheme {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Gpt => "GPT",
            Self::Mbr => "MBR",
        })
    }
}

impl std::str::FromStr for WindowsPartitionScheme {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "gpt" => Ok(Self::Gpt),
            "mbr" => Ok(Self::Mbr),
            _ => Err("expected gpt or mbr".into()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsExperienceOptions {
    pub bypass_hardware_requirements: bool,
    pub allow_offline_account: bool,
    pub local_account: Option<String>,
    pub regional: Option<WindowsRegionalOptions>,
    pub minimize_data_collection: bool,
    pub disable_bitlocker: bool,
    pub quality_of_life: bool,
    pub use_windows_ca_2023: bool,
    pub apply_skusi_policy: bool,
    pub force_s_mode: bool,
}

impl WindowsExperienceOptions {
    pub fn is_modified(&self) -> bool {
        self.bypass_hardware_requirements
            || self.allow_offline_account
            || self.local_account.is_some()
            || self.regional.is_some()
            || self.minimize_data_collection
            || self.disable_bitlocker
            || self.quality_of_life
            || self.use_windows_ca_2023
            || self.apply_skusi_policy
            || self.force_s_mode
    }

    pub fn requires_answer_file(&self) -> bool {
        self.bypass_hardware_requirements
            || self.allow_offline_account
            || self.local_account.is_some()
            || self.regional.is_some()
            || self.minimize_data_collection
            || self.disable_bitlocker
            || self.quality_of_life
            || self.apply_skusi_policy
            || self.force_s_mode
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsRegionalOptions {
    pub input_locale: String,
    pub system_locale: String,
    pub user_locale: String,
    pub ui_language: String,
    pub time_zone: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BadBlockCheck {
    #[default]
    Disabled,
    OnePass,
    TwoPasses,
    FourPasses,
}

impl BadBlockCheck {
    pub fn passes(self) -> usize {
        match self {
            Self::Disabled => 0,
            Self::OnePass => 1,
            Self::TwoPasses => 2,
            Self::FourPasses => 4,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Disabled => Self::OnePass,
            Self::OnePass => Self::TwoPasses,
            Self::TwoPasses => Self::FourPasses,
            Self::FourPasses => Self::Disabled,
        }
    }
}

impl fmt::Display for BadBlockCheck {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("off"),
            mode => write!(formatter, "{} pass(es)", mode.passes()),
        }
    }
}

impl FromStr for BadBlockCheck {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "off" | "disabled" | "0" => Ok(Self::Disabled),
            "1" | "one" => Ok(Self::OnePass),
            "2" | "two" => Ok(Self::TwoPasses),
            "4" | "four" => Ok(Self::FourPasses),
            _ => Err("use off, 1, 2, or 4".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgressPhase {
    Preparing,
    Downloading,
    Reading,
    Writing,
    Syncing,
    Verifying,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub phase: ProgressPhase,
    pub completed: u64,
    pub total: Option<u64>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivilegedWriteRequest {
    pub plan: WritePlan,
    pub confirmation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum PrivilegedWriteCommand {
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum PrivilegedWriteEvent {
    Progress(Progress),
    Finished,
    Failed { message: String },
}

impl Progress {
    pub fn ratio(&self) -> Option<f64> {
        self.total
            .filter(|total| *total > 0)
            .map(|total| (self.completed as f64 / total as f64).clamp(0.0, 1.0))
    }

    pub fn metrics(&self, elapsed: Duration) -> String {
        let elapsed_seconds = elapsed.as_secs_f64();
        let rate = (elapsed_seconds > 0.0 && self.completed > 0)
            .then_some(self.completed as f64 / elapsed_seconds);
        let mut parts = Vec::new();
        if let Some(ratio) = self.ratio() {
            parts.push(format!("{:.0}%", ratio * 100.0));
        }
        if let Some(total) = self.total {
            parts.push(format!(
                "{} / {}",
                format_bytes(self.completed.min(total)),
                format_bytes(total)
            ));
        } else if self.completed > 0 {
            parts.push(format_bytes(self.completed));
        }
        if let Some(rate) = rate {
            parts.push(format!("{}/s", format_bytes(rate as u64)));
            if let Some(total) = self.total.filter(|total| *total > self.completed) {
                let remaining = (total - self.completed) as f64 / rate;
                parts.push(format!("{} remaining", format_duration(remaining)));
            }
        }
        parts.push(format!("{} elapsed", format_duration(elapsed_seconds)));
        parts.join(" • ")
    }
}

impl fmt::Display for ProgressPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Preparing => "Preparing",
            Self::Downloading => "Downloading",
            Self::Reading => "Reading",
            Self::Writing => "Writing",
            Self::Syncing => "Syncing",
            Self::Verifying => "Verifying",
            Self::Finished => "Finished",
        })
    }
}

fn format_duration(seconds: f64) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn destructive_confirmation_ready(
    acknowledged: bool,
    write_active: bool,
    write_completed: bool,
) -> bool {
    acknowledged && !write_active && !write_completed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_block_modes_cycle_through_supported_pass_counts() {
        let mut mode = BadBlockCheck::Disabled;
        let mut passes = Vec::new();
        for _ in 0..4 {
            mode = mode.next();
            passes.push(mode.passes());
        }

        assert_eq!(passes, [1, 2, 4, 0]);
    }

    #[test]
    fn review_requires_an_image_and_an_eligible_removable_target() {
        let image = ImageReport {
            path: PathBuf::from("image.iso"),
            size: 1,
            kind: ImageKind::HybridIso,
            volume_label: None,
            warnings: Vec::new(),
        };
        let mut target = Device {
            id: DeviceId::new("usb"),
            path: PathBuf::from("/dev/test"),
            vendor: None,
            model: None,
            serial: None,
            transport: Some("usb".into()),
            capacity: 1,
            removable: true,
            read_only: false,
            system_disk: false,
            mounts: Vec::new(),
        };

        assert_eq!(
            review_readiness(None, Some(&target)),
            ReviewReadiness::NeedsImage
        );
        assert_eq!(
            review_readiness(Some(&image), None),
            ReviewReadiness::NeedsTarget
        );
        target.read_only = true;
        assert_eq!(
            review_readiness(Some(&image), Some(&target)),
            ReviewReadiness::NeedsTarget
        );
        target.read_only = false;
        assert_eq!(
            review_readiness(Some(&image), Some(&target)),
            ReviewReadiness::Ready
        );
    }

    #[test]
    fn progress_metrics_include_rate_eta_and_elapsed_time() {
        let progress = Progress {
            phase: ProgressPhase::Writing,
            completed: 50 * 1024 * 1024,
            total: Some(100 * 1024 * 1024),
            message: "Writing image".into(),
        };

        assert_eq!(
            progress.metrics(Duration::from_secs(10)),
            "50% • 50.0 MiB / 100.0 MiB • 5.0 MiB/s • 0:10 remaining • 0:10 elapsed"
        );
    }

    #[test]
    fn destructive_confirmation_requires_acknowledgment_and_one_inactive_attempt() {
        assert!(destructive_confirmation_ready(true, false, false));
        assert!(!destructive_confirmation_ready(false, false, false));
        assert!(!destructive_confirmation_ready(true, true, false));
        assert!(!destructive_confirmation_ready(true, false, true));
    }
}
