use crate::error::{Error, Result};
use crate::model::{
    Device, ImageKind, ImageReport, PlanStep, WriteOptions, WritePlan, WriteStrategy,
};

const WINDOWS_FREE_SPACE_ALLOWANCE: u64 = 256 * 1024 * 1024;

pub(crate) fn build(image: ImageReport, target: Device) -> Result<WritePlan> {
    build_with_options(image, target, WriteOptions::default())
}

pub(crate) fn build_with_options(
    image: ImageReport,
    target: Device,
    options: WriteOptions,
) -> Result<WritePlan> {
    validate_target(&target)?;
    if options.windows.is_modified() && !matches!(&image.kind, ImageKind::WindowsInstaller { .. }) {
        return Err(Error::UnsupportedImage(
            "Windows setup options apply only to Windows installer images".into(),
        ));
    }
    #[cfg(not(target_os = "linux"))]
    if options.bad_block_check.passes() > 0 {
        return Err(Error::PlatformUnavailable(
            "the destructive bad-block test is currently available only on Linux".into(),
        ));
    }
    #[cfg(target_os = "macos")]
    if matches!(&image.kind, ImageKind::WindowsInstaller { .. }) {
        return Err(Error::PlatformUnavailable(
            "native macOS Windows installer conversion is not implemented yet".into(),
        ));
    }
    let required = match image.kind {
        ImageKind::WindowsInstaller { .. } => {
            image.size.saturating_add(WINDOWS_FREE_SPACE_ALLOWANCE)
        }
        _ => image.size,
    };
    if required > target.capacity {
        return Err(Error::ImageTooLarge {
            required,
            available: target.capacity,
        });
    }

    let (strategy, mut steps, mut required_tools) = match image.kind {
        ImageKind::WindowsInstaller {
            payload,
            payload_size,
        } => {
            let split_payload = payload != crate::model::WindowsPayload::SplitWim
                && payload_size.is_some_and(|size| size > u32::MAX as u64);
            let tools = windows_required_tools(
                split_payload,
                options.windows.use_windows_ca_2023,
                options.windows_partition_scheme,
            );
            (
                WriteStrategy::WindowsFat32 {
                    payload,
                    split_payload,
                    partition_scheme: options.windows_partition_scheme,
                },
                windows_steps(&options),
                tools,
            )
        }
        ImageKind::HybridIso | ImageKind::RawDiskImage | ImageKind::CompressedDiskImage { .. } => (
            WriteStrategy::RawVerified,
            raw_steps(),
            raw_required_tools(),
        ),
        ImageKind::OpticalIso => {
            return Err(Error::UnsupportedImage(
                "optical-only ISOs need a conversion strategy before USB writing".into(),
            ));
        }
    };

    if options.bad_block_check.passes() > 0 {
        required_tools.push("badblocks".into());
        steps.insert(
            2.min(steps.len()),
            PlanStep {
                title: format!(
                    "Check the whole target with {} destructive test pattern(s)",
                    options.bad_block_check.passes()
                ),
                destructive: true,
            },
        );
    }

    let confirmation_phrase = confirmation_for(&target);
    Ok(WritePlan {
        image,
        target,
        strategy,
        options,
        steps,
        required_tools,
        confirmation_phrase,
    })
}

fn raw_required_tools() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        vec!["umount".into()]
    }
    #[cfg(target_os = "macos")]
    {
        vec!["diskutil".into()]
    }
    #[cfg(target_os = "windows")]
    {
        vec!["powershell.exe".into()]
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

fn windows_required_tools(
    split_payload: bool,
    use_windows_ca_2023: bool,
    partition_scheme: crate::model::WindowsPartitionScheme,
) -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        let mut tools = vec![
            "wipefs".into(),
            "partprobe".into(),
            "mkfs.fat".into(),
            "mount".into(),
            "umount".into(),
            "findmnt".into(),
            "sync".into(),
        ];
        tools.push(match partition_scheme {
            crate::model::WindowsPartitionScheme::Gpt => "sgdisk".into(),
            crate::model::WindowsPartitionScheme::Mbr => "parted".into(),
        });
        if split_payload || use_windows_ca_2023 {
            tools.push("wimlib-imagex".into());
        }
        tools
    }
    #[cfg(target_os = "windows")]
    {
        let _ = partition_scheme;
        let mut tools = vec!["powershell.exe".into()];
        if split_payload || use_windows_ca_2023 {
            tools.push("dism.exe".into());
        }
        tools
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (split_payload, use_windows_ca_2023, partition_scheme);
        Vec::new()
    }
}

fn validate_target(target: &Device) -> Result<()> {
    if target.system_disk {
        return Err(Error::UnsafeTarget(format!(
            "{} contains the running operating system",
            target.path.display()
        )));
    }
    if !target.removable {
        return Err(Error::UnsafeTarget(format!(
            "{} is not reported as removable or USB-attached",
            target.path.display()
        )));
    }
    if target.read_only {
        return Err(Error::UnsafeTarget(format!(
            "{} is read-only",
            target.path.display()
        )));
    }
    Ok(())
}

fn confirmation_for(target: &Device) -> String {
    let fingerprint = target
        .serial
        .as_deref()
        .unwrap_or_else(|| target.id.as_str());
    let suffix = fingerprint
        .chars()
        .rev()
        .take(8)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("ERASE {} {suffix}", target.path.display())
}

fn raw_steps() -> Vec<PlanStep> {
    vec![
        PlanStep {
            title: "Re-check the target's stable identity and safety flags".into(),
            destructive: false,
        },
        PlanStep {
            title: "Unmount target filesystems".into(),
            destructive: false,
        },
        PlanStep {
            title: "Stream the image to the entire device".into(),
            destructive: true,
        },
        PlanStep {
            title: "Flush writes and byte-verify the written image".into(),
            destructive: false,
        },
    ]
}

fn windows_steps(options: &WriteOptions) -> Vec<PlanStep> {
    let mut steps = vec![
        PlanStep {
            title: "Re-check the target's stable identity and safety flags".into(),
            destructive: false,
        },
        PlanStep {
            title: "Unmount target filesystems".into(),
            destructive: false,
        },
        PlanStep {
            title: match options.windows_partition_scheme {
                crate::model::WindowsPartitionScheme::Gpt => {
                    "Clear old signatures and create GPT with one Microsoft Basic Data partition"
                        .into()
                }
                crate::model::WindowsPartitionScheme::Mbr => {
                    "Clear old signatures and create MBR with one active FAT32 partition".into()
                }
            },
            destructive: true,
        },
        PlanStep {
            title: "Format FAT32 and copy the Windows installer".into(),
            destructive: true,
        },
        PlanStep {
            title: "Split install.wim/install.esd when it exceeds FAT32's file limit".into(),
            destructive: false,
        },
        PlanStep {
            title: "Verify UEFI boot files, payload chunks, and filesystem limits".into(),
            destructive: false,
        },
    ];
    if options.windows.bypass_hardware_requirements {
        steps.insert(
            steps.len() - 1,
            PlanStep {
                title: "Add Windows 11 TPM, Secure Boot, and RAM setup bypass".into(),
                destructive: false,
            },
        );
    }
    if options.windows.allow_offline_account {
        steps.insert(
            steps.len() - 1,
            PlanStep {
                title: "Expose the offline/local-account path during Windows OOBE".into(),
                destructive: false,
            },
        );
    }
    if let Some(account) = &options.windows.local_account {
        steps.insert(
            steps.len() - 1,
            PlanStep {
                title: format!("Create the Windows local administrator account `{account}`"),
                destructive: false,
            },
        );
    }
    if let Some(regional) = &options.windows.regional {
        steps.insert(
            steps.len() - 1,
            PlanStep {
                title: format!(
                    "Apply Windows locale {} and time zone {}",
                    regional.user_locale, regional.time_zone
                ),
                destructive: false,
            },
        );
    }
    if options.windows.minimize_data_collection {
        steps.insert(
            steps.len() - 1,
            PlanStep {
                title: "Apply privacy-focused Windows OOBE defaults".into(),
                destructive: false,
            },
        );
    }
    if options.windows.disable_bitlocker {
        steps.insert(
            steps.len() - 1,
            PlanStep {
                title: "Prevent automatic Windows device encryption".into(),
                destructive: false,
            },
        );
    }
    if options.windows.quality_of_life {
        steps.insert(
            steps.len() - 1,
            PlanStep {
                title: "Apply selected Windows QoL policies for bundled experiences".into(),
                destructive: false,
            },
        );
    }
    if options.windows.use_windows_ca_2023 {
        steps.insert(
            steps.len() - 1,
            PlanStep {
                title: "Replace media boot files with Windows UEFI CA 2023 signed versions".into(),
                destructive: false,
            },
        );
    }
    if options.windows.apply_skusi_policy {
        steps.insert(
            steps.len() - 1,
            PlanStep {
                title: "Schedule SkuSiPolicy.p7b Secure Boot revocations after installation".into(),
                destructive: false,
            },
        );
    }
    if options.windows.force_s_mode {
        steps.insert(
            steps.len() - 1,
            PlanStep {
                title: "Force Windows S Mode through offline servicing".into(),
                destructive: false,
            },
        );
    }
    steps
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::{DeviceId, WindowsPayload};

    fn device() -> Device {
        Device {
            id: DeviceId::new("serial:ABC123456"),
            path: PathBuf::from("/dev/sdz"),
            vendor: Some("SanDisk".into()),
            model: Some("Ultra".into()),
            serial: Some("ABC123456".into()),
            transport: Some("usb".into()),
            capacity: 32 * 1024 * 1024 * 1024,
            removable: true,
            read_only: false,
            system_disk: false,
            mounts: Vec::new(),
        }
    }

    fn image(kind: ImageKind) -> ImageReport {
        ImageReport {
            path: PathBuf::from("windows.iso"),
            size: 6 * 1024 * 1024 * 1024,
            kind,
            volume_label: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn windows_uses_basic_data_fat32_strategy() {
        let plan = build(
            image(ImageKind::WindowsInstaller {
                payload: WindowsPayload::Wim,
                payload_size: Some(6 * 1024 * 1024 * 1024),
            }),
            device(),
        )
        .expect("valid plan");

        assert!(matches!(
            plan.strategy,
            WriteStrategy::WindowsFat32 {
                payload: WindowsPayload::Wim,
                split_payload: true,
                partition_scheme: crate::model::WindowsPartitionScheme::Gpt,
            }
        ));
        assert_eq!(plan.confirmation_phrase, "ERASE /dev/sdz BC123456");
        assert!(plan.confirmation_matches("ERASE /dev/sdz BC123456"));
        assert!(!plan.confirmation_matches("erase /dev/sdz BC123456"));
        assert!(!plan.confirmation_matches("ERASE /dev/sdz BC123456 "));
    }

    #[test]
    fn windows_mbr_uefi_plan_uses_parted_and_active_partition() {
        let options = WriteOptions {
            windows_partition_scheme: crate::model::WindowsPartitionScheme::Mbr,
            ..WriteOptions::default()
        };
        let plan = build_with_options(
            image(ImageKind::WindowsInstaller {
                payload: WindowsPayload::Esd,
                payload_size: Some(3 * 1024 * 1024 * 1024),
            }),
            device(),
            options,
        )
        .expect("valid MBR plan");

        assert!(matches!(
            plan.strategy,
            WriteStrategy::WindowsFat32 {
                partition_scheme: crate::model::WindowsPartitionScheme::Mbr,
                ..
            }
        ));
        assert!(plan.required_tools.iter().any(|tool| tool == "parted"));
        assert!(!plan.required_tools.iter().any(|tool| tool == "sgdisk"));
        assert!(
            plan.steps
                .iter()
                .any(|step| step.title.contains("active FAT32"))
        );
    }

    #[test]
    fn windows_requirement_bypass_is_explicit_in_the_plan() {
        let mut options = WriteOptions::default();
        options.windows.bypass_hardware_requirements = true;
        let plan = build_with_options(
            image(ImageKind::WindowsInstaller {
                payload: WindowsPayload::Wim,
                payload_size: Some(3 * 1024 * 1024 * 1024),
            }),
            device(),
            options,
        )
        .expect("valid Windows plan");

        assert!(plan.options.windows.bypass_hardware_requirements);
        assert!(
            plan.steps
                .iter()
                .any(|step| step.title.contains("TPM, Secure Boot, and RAM"))
        );
    }

    #[test]
    fn windows_options_are_rejected_for_non_windows_images() {
        let mut options = WriteOptions::default();
        options.windows.bypass_hardware_requirements = true;

        let error = build_with_options(image(ImageKind::HybridIso), device(), options)
            .expect_err("incompatible options");

        assert!(matches!(error, Error::UnsupportedImage(_)));
    }

    #[test]
    fn bad_block_check_is_in_the_reviewed_plan() {
        let options = WriteOptions {
            bad_block_check: crate::model::BadBlockCheck::TwoPasses,
            ..WriteOptions::default()
        };

        let plan = build_with_options(image(ImageKind::HybridIso), device(), options)
            .expect("valid checked write");

        assert!(plan.required_tools.iter().any(|tool| tool == "badblocks"));
        assert!(
            plan.steps
                .iter()
                .any(|step| step.title.contains("2 destructive test pattern"))
        );
    }

    #[test]
    fn refuses_a_system_disk() {
        let mut target = device();
        target.system_disk = true;

        let error = build(image(ImageKind::HybridIso), target).expect_err("unsafe target");

        assert!(matches!(error, Error::UnsafeTarget(_)));
    }

    #[test]
    fn refuses_optical_only_images() {
        let error = build(image(ImageKind::OpticalIso), device()).expect_err("unsupported");
        assert!(matches!(error, Error::UnsupportedImage(_)));
    }
}
