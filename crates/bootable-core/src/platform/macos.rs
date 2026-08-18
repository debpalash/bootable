use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;

use serde::Deserialize;

use crate::error::{Error, Result, io_error};
use crate::model::{
    Device, DeviceId, Progress, ProgressPhase, WindowsExperienceOptions, WindowsPartitionScheme,
    WindowsPayload, WritePlan, WriteStrategy,
};
use crate::operation::OperationControl;
use crate::windows;

const DISKUTIL: &str = "/usr/sbin/diskutil";
const HDIUTIL: &str = "/usr/bin/hdiutil";
const ID: &str = "/usr/bin/id";
const PLUTIL: &str = "/usr/bin/plutil";
const SYNC: &str = "/bin/sync";
const BUFFER_SIZE: usize = 4 * 1024 * 1024;
const FAT32_MAX_FILE_SIZE: u64 = u32::MAX as u64;
const WIM_CHUNK_MIB: &str = "3800";

pub(crate) struct NativePlatform;

impl NativePlatform {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn devices(&self) -> Result<Vec<Device>> {
        discover_devices()
    }

    pub(crate) fn inspect_override(
        &self,
        _path: &Path,
    ) -> Option<Result<crate::model::ImageReport>> {
        None
    }

    pub(crate) fn write(
        &self,
        plan: &WritePlan,
        confirmation: &str,
        control: &OperationControl,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        control.checkpoint()?;
        if !plan.confirmation_matches(confirmation) {
            return Err(Error::ConfirmationMismatch {
                expected: plan.confirmation_phrase.clone(),
            });
        }
        if !is_root()? {
            return Err(Error::NotPrivileged);
        }
        let target = refresh_target(plan)?;
        let buffered_path = buffered_disk_path(&target.path)?;
        run_diskutil(["unmountDisk", buffered_path.as_str()])?;
        control.checkpoint()?;
        match plan.strategy {
            WriteStrategy::RawVerified => {
                super::raw::write(&plan.image, &target.path, control, progress)
            }
            WriteStrategy::WindowsFat32 {
                payload,
                partition_scheme,
                ..
            } => windows_write(plan, &target, payload, partition_scheme, control, progress),
        }
    }

    pub(crate) fn backup(
        &self,
        device_id_or_path: &str,
        destination: &Path,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        if !is_root()? {
            return Err(Error::NotPrivileged);
        }
        let source = discover_devices()?
            .into_iter()
            .find(|device| {
                device.id.as_str() == device_id_or_path
                    || device.path.to_string_lossy() == device_id_or_path
            })
            .ok_or_else(|| Error::DeviceNotFound(device_id_or_path.into()))?;
        super::raw::backup(&source.path, source.capacity, destination, progress)
    }
}

#[derive(Debug, Default, Deserialize)]
struct IoMedia {
    #[serde(rename = "IORegistryEntryID")]
    registry_id: Option<u64>,
    #[serde(rename = "BSD Name")]
    bsd_name: Option<String>,
    #[serde(rename = "Whole", default)]
    whole: bool,
    #[serde(rename = "Removable", default)]
    removable: bool,
    #[serde(rename = "Ejectable", default)]
    ejectable: bool,
    #[serde(rename = "Writable", default)]
    writable: bool,
    #[serde(rename = "Size", default)]
    size: u64,
    #[serde(rename = "Device Characteristics", default)]
    device: DeviceCharacteristics,
    #[serde(rename = "Protocol Characteristics", default)]
    protocol: ProtocolCharacteristics,
}

#[derive(Debug, Default, Deserialize)]
struct DeviceCharacteristics {
    #[serde(rename = "Serial Number")]
    serial: Option<String>,
    #[serde(rename = "Product Name")]
    product: Option<String>,
    #[serde(rename = "Vendor Name")]
    vendor: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ProtocolCharacteristics {
    #[serde(rename = "Physical Interconnect")]
    interconnect: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskInfo {
    device_identifier: Option<String>,
    part_of_whole: Option<String>,
    mount_point: Option<PathBuf>,
    file_system_personality: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskList {
    #[serde(default)]
    all_disks_and_partitions: Vec<DiskListDisk>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskListDisk {
    device_identifier: Option<String>,
    #[serde(default)]
    partitions: Vec<DiskListPartition>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiskListPartition {
    device_identifier: Option<String>,
    volume_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct HdiutilAttach {
    #[serde(default)]
    system_entities: Vec<HdiutilEntity>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct HdiutilEntity {
    dev_entry: Option<PathBuf>,
    mount_point: Option<PathBuf>,
}

#[derive(Debug)]
struct MountedImage {
    device: PathBuf,
    mount_point: PathBuf,
}

fn windows_write(
    plan: &WritePlan,
    target: &Device,
    payload: WindowsPayload,
    partition_scheme: WindowsPartitionScheme,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let source = attach_iso(&plan.image.path)?;
    let buffered_path = buffered_disk_path(&target.path)?;
    let mut target_prepared = false;
    let result = (|| {
        let payload_path =
            preflight_windows_source(&source.mount_point, payload, &plan.options.windows, control)?;
        control.checkpoint()?;
        progress(Progress {
            phase: ProgressPhase::Preparing,
            completed: 0,
            total: Some(plan.image.size),
            message: format!("Creating {partition_scheme} Windows FAT32 media"),
        });
        let scheme = match partition_scheme {
            WindowsPartitionScheme::Gpt => "GPT",
            WindowsPartitionScheme::Mbr => "MBRFormat",
        };
        run_diskutil([
            "eraseDisk",
            "MS-DOS",
            "WINDOWS",
            scheme,
            buffered_path.as_str(),
        ])?;
        target_prepared = true;
        let destination = wait_for_windows_volume(&buffered_path)?;
        copy_windows_tree(
            &source.mount_point,
            &destination,
            &payload_path,
            payload,
            plan.image.size,
            control,
            progress,
        )?;
        apply_windows_options(&destination, &plan.options.windows)?;
        if plan.options.windows.use_windows_ca_2023 {
            apply_windows_ca_2023(&destination, control)?;
        }
        verify_windows_tree(&destination)?;
        run_status(SYNC, std::iter::empty::<&OsStr>())?;
        Ok(())
    })();

    let target_cleanup = if target_prepared {
        run_diskutil(["unmountDisk", buffered_path.as_str()])
    } else {
        Ok(())
    };
    let source_cleanup = detach_iso(&source.device);
    result?;
    target_cleanup?;
    source_cleanup?;
    progress(Progress {
        phase: ProgressPhase::Finished,
        completed: plan.image.size,
        total: Some(plan.image.size),
        message: format!("Windows installer ready on {partition_scheme} FAT32 media"),
    });
    Ok(())
}

fn attach_iso(image: &Path) -> Result<MountedImage> {
    let output = Command::new(HDIUTIL)
        .args(["attach", "-readonly", "-nobrowse", "-plist"])
        .arg(image)
        .output()
        .map_err(|error| io_error(HDIUTIL, error))?;
    if !output.status.success() {
        return Err(command_failed(HDIUTIL, &output));
    }
    let attached: HdiutilAttach = plist_json(&output.stdout, "hdiutil attach")?;
    let mounted = mounted_image_from_attach(attached)?;
    validate_hdi_device(&mounted.device)?;
    validate_volume_mount(&mounted.mount_point)?;
    Ok(mounted)
}

fn mounted_image_from_attach(attached: HdiutilAttach) -> Result<MountedImage> {
    attached
        .system_entities
        .into_iter()
        .find_map(|entity| {
            Some(MountedImage {
                device: entity.dev_entry?,
                mount_point: entity.mount_point?,
            })
        })
        .ok_or_else(|| Error::InvalidToolOutput {
            program: "hdiutil attach".into(),
            message: "the mounted image has no filesystem mount point".into(),
        })
}

fn validate_hdi_device(device: &Path) -> Result<()> {
    let Some(value) = device
        .to_str()
        .and_then(|value| value.strip_prefix("/dev/disk"))
    else {
        return Err(Error::InvalidToolOutput {
            program: "hdiutil attach".into(),
            message: format!("unexpected image device {}", device.display()),
        });
    };
    let valid = value.split_once('s').map_or_else(
        || !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
        |(disk, slice)| {
            !disk.is_empty()
                && !slice.is_empty()
                && disk.bytes().all(|byte| byte.is_ascii_digit())
                && slice.bytes().all(|byte| byte.is_ascii_digit())
        },
    );
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidToolOutput {
            program: "hdiutil attach".into(),
            message: format!("unexpected image device {}", device.display()),
        })
    }
}

fn validate_volume_mount(path: &Path) -> Result<PathBuf> {
    let canonical = path.canonicalize().map_err(|error| io_error(path, error))?;
    if canonical.is_dir() && canonical.starts_with("/Volumes") {
        Ok(canonical)
    } else {
        Err(Error::UnsafeTarget(format!(
            "macOS returned an unexpected volume mount point: {}",
            path.display()
        )))
    }
}

fn detach_iso(device: &Path) -> Result<()> {
    run_status(HDIUTIL, [OsStr::new("detach"), device.as_os_str()])
}

fn wait_for_windows_volume(disk: &str) -> Result<PathBuf> {
    let disk_path = Path::new(disk);
    let disk_identifier = disk_path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| Error::UnsafeTarget(format!("unexpected disk path: {disk}")))?;
    validate_whole_disk_identifier(disk_identifier)?;
    for _ in 0..50 {
        if let Ok(list) = disk_list(disk_path)
            && let Some(partition) = windows_partition_from_list(&list, disk_identifier)?
            && let Ok(info) = disk_info(&partition)
            && info.part_of_whole.as_deref() == Some(disk_identifier)
            && info
                .device_identifier
                .as_deref()
                .is_some_and(|identifier| identifier == partition.file_name().unwrap_or_default())
            && info
                .file_system_personality
                .as_deref()
                .is_none_or(|filesystem| {
                    filesystem.eq_ignore_ascii_case("MS-DOS FAT32")
                        || filesystem.eq_ignore_ascii_case("FAT32")
                })
            && let Some(mount) = info.mount_point
        {
            return validate_volume_mount(&mount);
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(Error::DeviceNotFound(format!(
        "the FAT32 partition on {disk} did not become available"
    )))
}

fn validate_whole_disk_identifier(identifier: &str) -> Result<()> {
    let suffix = identifier.strip_prefix("disk").unwrap_or_default();
    if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(())
    } else {
        Err(Error::UnsafeTarget(format!(
            "unexpected whole-disk identifier: {identifier}"
        )))
    }
}

fn windows_partition_from_list(list: &DiskList, disk_identifier: &str) -> Result<Option<PathBuf>> {
    validate_whole_disk_identifier(disk_identifier)?;
    let Some(disk) = list
        .all_disks_and_partitions
        .iter()
        .find(|disk| disk.device_identifier.as_deref() == Some(disk_identifier))
    else {
        return Ok(None);
    };
    let Some(identifier) = disk
        .partitions
        .iter()
        .find(|partition| partition.volume_name.as_deref() == Some("WINDOWS"))
        .and_then(|partition| partition.device_identifier.as_deref())
    else {
        return Ok(None);
    };
    let Some(slice) = identifier
        .strip_prefix(disk_identifier)
        .and_then(|value| value.strip_prefix('s'))
    else {
        return Err(Error::StalePlan(format!(
            "diskutil associated the WINDOWS volume with unexpected partition {identifier}"
        )));
    };
    if slice.is_empty() || !slice.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::StalePlan(format!(
            "diskutil returned an invalid WINDOWS partition identifier: {identifier}"
        )));
    }
    Ok(Some(PathBuf::from(format!("/dev/{identifier}"))))
}

fn disk_list(path: &Path) -> Result<DiskList> {
    let output = Command::new(DISKUTIL)
        .args(["list", "-plist"])
        .arg(path)
        .output()
        .map_err(|error| io_error(DISKUTIL, error))?;
    if !output.status.success() {
        return Err(command_failed(DISKUTIL, &output));
    }
    plist_json(&output.stdout, "diskutil list")
}

fn disk_info(path: &Path) -> Result<DiskInfo> {
    let output = Command::new(DISKUTIL)
        .args(["info", "-plist"])
        .arg(path)
        .output()
        .map_err(|error| io_error(DISKUTIL, error))?;
    if !output.status.success() {
        return Err(command_failed(DISKUTIL, &output));
    }
    plist_json(&output.stdout, "diskutil info")
}

fn preflight_windows_source(
    source: &Path,
    payload: WindowsPayload,
    options: &WindowsExperienceOptions,
    control: &OperationControl,
) -> Result<PathBuf> {
    control.checkpoint()?;
    if options.requires_answer_file() {
        windows::answer_file(options)?;
        if find_optional_case_insensitive_child(source, "autounattend.xml").is_some() {
            return Err(Error::UnsupportedImage(
                "the source already contains autounattend.xml; refusing to overwrite it".into(),
            ));
        }
    }
    let efi = find_case_insensitive_child(source, "efi")?;
    let efi_boot = find_case_insensitive_child(&efi, "boot")?;
    let has_efi_loader = fs::read_dir(&efi_boot)
        .map_err(|error| io_error(&efi_boot, error))?
        .filter_map(std::result::Result::ok)
        .any(|entry| {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            name.starts_with("boot") && name.ends_with(".efi")
        });
    if !has_efi_loader {
        return Err(Error::UnsupportedImage(
            "Windows source is missing efi/boot/boot*.efi".into(),
        ));
    }
    find_case_insensitive_child(source, "bootmgr")?;
    let sources = find_case_insensitive_child(source, "sources")?;
    find_case_insensitive_child(&sources, "boot.wim")?;
    let payload_path = find_install_payload(source, payload)?;
    let payload_size = payload_path
        .metadata()
        .map_err(|error| io_error(&payload_path, error))?
        .len();
    if payload_size > FAT32_MAX_FILE_SIZE || options.use_windows_ca_2023 {
        let _ = wimlib_path()?;
    }
    reject_other_oversized_files(source, &payload_path)?;
    Ok(payload_path)
}

fn reject_other_oversized_files(path: &Path, payload: &Path) -> Result<()> {
    for entry in fs::read_dir(path).map_err(|error| io_error(path, error))? {
        let entry = entry.map_err(|error| io_error(path, error))?;
        let child = entry.path();
        if child == payload {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| io_error(&child, error))?;
        if file_type.is_dir() {
            reject_other_oversized_files(&child, payload)?;
        } else if file_type.is_file()
            && entry
                .metadata()
                .map_err(|error| io_error(&child, error))?
                .len()
                > FAT32_MAX_FILE_SIZE
        {
            return Err(Error::UnsupportedImage(format!(
                "{} exceeds FAT32's file limit and is not the splittable install payload",
                child.display()
            )));
        } else if !file_type.is_file() {
            return Err(Error::UnsupportedImage(format!(
                "{} is not a regular Windows installation file",
                child.display()
            )));
        }
    }
    Ok(())
}

fn apply_windows_options(root: &Path, options: &WindowsExperienceOptions) -> Result<()> {
    if !options.requires_answer_file() {
        return Ok(());
    }
    let path = root.join("autounattend.xml");
    if path.exists() {
        return Err(Error::UnsupportedImage(
            "the source already contains autounattend.xml; refusing to overwrite it".into(),
        ));
    }
    fs::write(&path, windows::answer_file(options)?).map_err(|error| io_error(path, error))
}

fn apply_windows_ca_2023(root: &Path, control: &OperationControl) -> Result<()> {
    let wimlib = wimlib_path()?;
    let sources = find_case_insensitive_child(root, "sources")?;
    let boot_wim = find_case_insensitive_child(&sources, "boot.wim")?;
    let extracted = tempfile::tempdir().map_err(|error| io_error("temporary directory", error))?;
    let destination = format!("--dest-dir={}", extracted.path().display());
    run_status_controlled(
        &wimlib,
        [
            OsString::from("extract"),
            boot_wim.into_os_string(),
            OsString::from("2"),
            OsString::from("Windows/Boot/EFI_EX"),
            OsString::from("Windows/Boot/Fonts_EX"),
            OsString::from(destination),
            OsString::from("--no-acls"),
        ],
        control,
    )?;

    let efi_ex = extracted.path().join("EFI_EX");
    let fonts_ex = extracted.path().join("Fonts_EX");
    let boot_manager = efi_ex.join("bootmgfw_EX.efi");
    let root_manager = efi_ex.join("bootmgr_EX.efi");
    if !boot_manager.is_file() || !root_manager.is_file() {
        return Err(Error::UnsupportedImage(
            "the Windows image does not contain CA 2023 EFI_EX bootloaders".into(),
        ));
    }
    let efi = find_case_insensitive_child(root, "efi")?;
    let boot_directory = find_case_insensitive_child(&efi, "boot")?;
    let fallback = fs::read_dir(&boot_directory)
        .map_err(|error| io_error(&boot_directory, error))?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| {
                    let name = name.to_ascii_lowercase();
                    name.starts_with("boot") && name.ends_with(".efi")
                })
        })
        .ok_or_else(|| {
            Error::UnsupportedImage("Windows media has no UEFI fallback bootloader".into())
        })?;
    fs::copy(&boot_manager, &fallback).map_err(|error| io_error(&fallback, error))?;
    let root_boot_manager = root.join("bootmgr.efi");
    fs::copy(&root_manager, &root_boot_manager)
        .map_err(|error| io_error(&root_boot_manager, error))?;
    let microsoft = find_case_insensitive_child(&efi, "microsoft")?;
    let microsoft_boot = find_case_insensitive_child(&microsoft, "boot")?;
    let fonts = find_case_insensitive_child(&microsoft_boot, "fonts")?;
    copy_ca_2023_fonts(&fonts_ex, &fonts, &fonts_ex)
}

fn copy_ca_2023_fonts(source: &Path, destination: &Path, root: &Path) -> Result<()> {
    for entry in fs::read_dir(source).map_err(|error| io_error(source, error))? {
        let entry = entry.map_err(|error| io_error(source, error))?;
        let path = entry.path();
        if path.is_dir() {
            copy_ca_2023_fonts(&path, destination, root)?;
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            Error::UnsupportedImage("CA 2023 font extraction escaped its workspace".into())
        })?;
        let relative = PathBuf::from(relative.to_string_lossy().replace("_EX", ""));
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        fs::copy(&path, &target).map_err(|error| io_error(&target, error))?;
    }
    Ok(())
}

fn copy_windows_tree(
    source: &Path,
    destination: &Path,
    payload_path: &Path,
    payload: WindowsPayload,
    total: u64,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let payload_size = payload_path
        .metadata()
        .map_err(|error| io_error(payload_path, error))?
        .len();
    let mut copied = 0_u64;
    copy_tree(
        source,
        destination,
        Some(payload_path),
        &mut copied,
        total,
        control,
        progress,
    )?;
    if payload_size > FAT32_MAX_FILE_SIZE {
        if payload == WindowsPayload::SplitWim {
            return Err(Error::UnsupportedImage(
                "a pre-split install.swm part exceeds FAT32's file-size limit".into(),
            ));
        }
        let sources = find_case_insensitive_child(destination, "sources")?;
        let split = sources.join("install.swm");
        run_status_controlled(
            &wimlib_path()?,
            [
                OsString::from("split"),
                payload_path.as_os_str().to_owned(),
                split.as_os_str().to_owned(),
                OsString::from(WIM_CHUNK_MIB),
            ],
            control,
        )?;
        verify_split_wim(destination, control)?;
    } else {
        let relative = payload_path.strip_prefix(source).map_err(|_| {
            Error::UnsupportedImage("Windows payload escaped the mounted ISO".into())
        })?;
        copy_file(
            payload_path,
            &destination.join(relative),
            &mut copied,
            total,
            control,
            progress,
        )?;
    }
    Ok(())
}

fn verify_split_wim(root: &Path, control: &OperationControl) -> Result<()> {
    let sources = find_case_insensitive_child(root, "sources")?;
    let first = find_case_insensitive_child(&sources, "install.swm")?;
    let mut reference = OsString::from("--ref=");
    reference.push(sources.join("install*.swm"));
    run_status_controlled(
        &wimlib_path()?,
        [OsString::from("verify"), first.into_os_string(), reference],
        control,
    )
}

fn copy_tree(
    source: &Path,
    destination: &Path,
    skipped: Option<&Path>,
    copied: &mut u64,
    total: u64,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    control.checkpoint()?;
    fs::create_dir_all(destination).map_err(|error| io_error(destination, error))?;
    for entry in fs::read_dir(source).map_err(|error| io_error(source, error))? {
        let entry = entry.map_err(|error| io_error(source, error))?;
        let source_path = entry.path();
        if skipped == Some(source_path.as_path()) {
            continue;
        }
        let destination_path = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| io_error(&source_path, error))?;
        if file_type.is_dir() {
            copy_tree(
                &source_path,
                &destination_path,
                skipped,
                copied,
                total,
                control,
                progress,
            )?;
        } else if file_type.is_file() {
            copy_file(
                &source_path,
                &destination_path,
                copied,
                total,
                control,
                progress,
            )?;
        } else {
            return Err(Error::UnsupportedImage(format!(
                "{} is not a regular Windows installation file",
                source_path.display()
            )));
        }
    }
    Ok(())
}

fn copy_file(
    source: &Path,
    destination: &Path,
    copied: &mut u64,
    total: u64,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    control.checkpoint()?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    let mut reader = BufReader::with_capacity(
        BUFFER_SIZE,
        File::open(source).map_err(|error| io_error(source, error))?,
    );
    let mut writer = BufWriter::with_capacity(
        BUFFER_SIZE,
        File::create(destination).map_err(|error| io_error(destination, error))?,
    );
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        control.checkpoint()?;
        let count = reader
            .read(&mut buffer)
            .map_err(|error| io_error(source, error))?;
        if count == 0 {
            break;
        }
        writer
            .write_all(&buffer[..count])
            .map_err(|error| io_error(destination, error))?;
        *copied = copied.saturating_add(count as u64);
        progress(Progress {
            phase: ProgressPhase::Writing,
            completed: (*copied).min(total),
            total: Some(total),
            message: format!("Copying {}", source.display()),
        });
    }
    writer.flush().map_err(|error| io_error(destination, error))
}

fn find_install_payload(root: &Path, payload: WindowsPayload) -> Result<PathBuf> {
    let sources = find_case_insensitive_child(root, "sources")?;
    find_case_insensitive_child(
        &sources,
        match payload {
            WindowsPayload::Wim => "install.wim",
            WindowsPayload::Esd => "install.esd",
            WindowsPayload::SplitWim => "install.swm",
        },
    )
}

fn find_case_insensitive_child(parent: &Path, name: &str) -> Result<PathBuf> {
    find_optional_case_insensitive_child(parent, name).ok_or_else(|| {
        Error::UnsupportedImage(format!("missing {name} below {}", parent.display()))
    })
}

fn find_optional_case_insensitive_child(parent: &Path, name: &str) -> Option<PathBuf> {
    fs::read_dir(parent)
        .ok()?
        .filter_map(std::result::Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
        })
        .map(|entry| entry.path())
}

fn verify_windows_tree(root: &Path) -> Result<()> {
    let efi = find_case_insensitive_child(root, "efi")?;
    let efi_boot = find_case_insensitive_child(&efi, "boot")?;
    let has_efi_loader = fs::read_dir(&efi_boot)
        .map_err(|error| io_error(&efi_boot, error))?
        .filter_map(std::result::Result::ok)
        .any(|entry| {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            name.starts_with("boot") && name.ends_with(".efi")
        });
    if !has_efi_loader {
        return Err(Error::UnsupportedImage(
            "written media is missing efi/boot/boot*.efi".into(),
        ));
    }
    find_case_insensitive_child(root, "bootmgr")?;
    let sources = find_case_insensitive_child(root, "sources")?;
    find_case_insensitive_child(&sources, "boot.wim")?;
    let has_install_payload = fs::read_dir(&sources)
        .map_err(|error| io_error(&sources, error))?
        .filter_map(std::result::Result::ok)
        .any(|entry| {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            name == "install.wim"
                || name == "install.esd"
                || (name.starts_with("install") && name.ends_with(".swm"))
        });
    if !has_install_payload {
        return Err(Error::UnsupportedImage(
            "written media is missing its Windows install payload".into(),
        ));
    }
    verify_fat_file_sizes(root)
}

fn verify_fat_file_sizes(path: &Path) -> Result<()> {
    for entry in fs::read_dir(path).map_err(|error| io_error(path, error))? {
        let entry = entry.map_err(|error| io_error(path, error))?;
        let child = entry.path();
        let metadata = entry.metadata().map_err(|error| io_error(&child, error))?;
        if metadata.is_dir() {
            verify_fat_file_sizes(&child)?;
        } else if metadata.len() > FAT32_MAX_FILE_SIZE {
            return Err(Error::UnsupportedImage(format!(
                "{} exceeds FAT32's maximum file size",
                child.display()
            )));
        }
    }
    Ok(())
}

fn wimlib_path() -> Result<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin/wimlib-imagex"),
        PathBuf::from("/usr/local/bin/wimlib-imagex"),
        PathBuf::from("/usr/bin/wimlib-imagex"),
    ];
    if let Some(paths) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&paths).map(|path| path.join("wimlib-imagex")));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or(Error::MissingTool("wimlib-imagex"))
}

fn run_status<I, S>(program: &'static str, arguments: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| io_error(program, error))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_failed(program, &output))
    }
}

fn run_status_controlled<I, S>(
    program: &Path,
    arguments: I,
    control: &OperationControl,
) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    control.checkpoint()?;
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| io_error(program, error))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::InvalidToolOutput {
            program: program.display().to_string(),
            message: "command did not provide stdout".into(),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::InvalidToolOutput {
            program: program.display().to_string(),
            message: "command did not provide stderr".into(),
        })?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = BufReader::new(stdout).read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = BufReader::new(stderr).read_to_end(&mut bytes);
        bytes
    });
    let status = loop {
        if let Err(error) = control.checkpoint() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(error);
        }
        if let Some(status) = child.try_wait().map_err(|error| io_error(program, error))? {
            break status;
        }
        thread::sleep(Duration::from_millis(100));
    };
    let _stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(Error::CommandFailed {
            program: program.display().to_string(),
            message: String::from_utf8_lossy(&stderr).trim().to_owned(),
        })
    }
}

fn command_failed(program: &str, output: &Output) -> Error {
    Error::CommandFailed {
        program: program.into(),
        message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    }
}

fn discover_devices() -> Result<Vec<Device>> {
    let root_disk = root_disk()?;
    let output = Command::new("/usr/sbin/ioreg")
        .args(["-r", "-c", "IOMedia", "-a"])
        .output()
        .map_err(|error| io_error("ioreg", error))?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            program: "ioreg".into(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let media: Vec<IoMedia> = plist_json(&output.stdout, "ioreg")?;
    Ok(devices_from_media(media, root_disk.as_deref()))
}

fn devices_from_media(media: Vec<IoMedia>, root_disk: Option<&str>) -> Vec<Device> {
    media
        .into_iter()
        .filter(|media| {
            media.whole
                && media.size > 0
                && (media.removable || media.ejectable)
                && media.bsd_name.as_deref() != root_disk
        })
        .filter_map(|media| {
            let name = media.bsd_name?;
            let serial = clean(media.device.serial);
            let id = if let Some(value) = serial.as_deref() {
                DeviceId::new(format!("serial:{value}"))
            } else {
                let value = media.registry_id?;
                DeviceId::new(format!("macos-registry:{value}"))
            };
            Some(Device {
                id,
                path: PathBuf::from(format!("/dev/r{name}")),
                vendor: clean(media.device.vendor),
                model: clean(media.device.product),
                serial,
                transport: clean(media.protocol.interconnect),
                capacity: media.size,
                removable: true,
                read_only: !media.writable,
                system_disk: false,
                mounts: Vec::new(),
            })
        })
        .collect()
}

fn root_disk() -> Result<Option<String>> {
    let output = Command::new(DISKUTIL)
        .args(["info", "-plist", "/"])
        .output()
        .map_err(|error| io_error("diskutil", error))?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            program: "diskutil".into(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let info: DiskInfo = plist_json(&output.stdout, "diskutil info /")?;
    Ok(info.part_of_whole)
}

fn refresh_target(plan: &WritePlan) -> Result<Device> {
    let target = discover_devices()?
        .into_iter()
        .find(|device| device.id == plan.target.id)
        .ok_or_else(|| Error::DeviceNotFound(plan.target.id.to_string()))?;
    if target.path != plan.target.path || target.capacity != plan.target.capacity {
        return Err(Error::StalePlan(
            "the selected macOS disk identity, path, or capacity changed".into(),
        ));
    }
    if !target.is_eligible_target() {
        return Err(Error::StalePlan(
            "the refreshed macOS disk no longer passes removable-drive safety checks".into(),
        ));
    }
    Ok(target)
}

fn buffered_disk_path(raw_path: &Path) -> Result<String> {
    raw_path
        .to_str()
        .and_then(|value| value.strip_prefix("/dev/rdisk"))
        .filter(|value| {
            !value.is_empty() && value.chars().all(|character| character.is_ascii_digit())
        })
        .map(|number| format!("/dev/disk{number}"))
        .ok_or_else(|| {
            Error::UnsafeTarget(format!("unexpected raw-disk path: {}", raw_path.display()))
        })
}

fn is_root() -> Result<bool> {
    let output = Command::new(ID)
        .arg("-u")
        .output()
        .map_err(|error| io_error("id", error))?;
    Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "0")
}

fn run_diskutil<const N: usize>(arguments: [&str; N]) -> Result<()> {
    let output = Command::new(DISKUTIL)
        .args(arguments)
        .output()
        .map_err(|error| io_error("diskutil", error))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::CommandFailed {
            program: "diskutil".into(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

fn plist_json<T: for<'de> Deserialize<'de>>(plist: &[u8], program: &str) -> Result<T> {
    let mut child = Command::new(PLUTIL)
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| io_error("plutil", error))?;
    child
        .stdin
        .take()
        .ok_or_else(|| Error::InvalidToolOutput {
            program: "plutil".into(),
            message: "missing input pipe".into(),
        })?
        .write_all(plist)
        .map_err(|error| io_error("plutil input", error))?;
    let output = child
        .wait_with_output()
        .map_err(|error| io_error("plutil", error))?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            program: "plutil".into(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|error| Error::InvalidToolOutput {
        program: program.into(),
        message: error.to_string(),
    })
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows_fixture() -> tempfile::TempDir {
        let fixture = tempfile::tempdir().expect("fixture");
        fs::create_dir_all(fixture.path().join("efi/boot")).expect("efi tree");
        fs::create_dir_all(fixture.path().join("sources")).expect("sources tree");
        fs::write(fixture.path().join("efi/boot/bootx64.efi"), b"efi").expect("fallback loader");
        fs::write(fixture.path().join("bootmgr"), b"bootmgr").expect("boot manager");
        fs::write(fixture.path().join("sources/boot.wim"), b"boot wim").expect("boot wim");
        fs::write(fixture.path().join("sources/install.wim"), b"install wim").expect("install wim");
        fixture
    }

    #[test]
    fn raw_disk_paths_are_strictly_validated() {
        assert_eq!(
            buffered_disk_path(Path::new("/dev/rdisk12")).expect("buffered path"),
            "/dev/disk12"
        );
        assert!(buffered_disk_path(Path::new("/dev/disk12")).is_err());
        assert!(buffered_disk_path(Path::new("/dev/rdisk1;rm")).is_err());
    }

    #[test]
    fn partition_inventory_and_attached_image_devices_are_strictly_validated() {
        let list: DiskList = serde_json::from_str(
            r#"{
                "AllDisksAndPartitions": [{
                    "DeviceIdentifier":"disk12",
                    "Partitions":[
                        {"DeviceIdentifier":"disk12s1","VolumeName":"EFI"},
                        {"DeviceIdentifier":"disk12s2","VolumeName":"WINDOWS"}
                    ]
                }]
            }"#,
        )
        .expect("diskutil list fixture");
        assert_eq!(
            windows_partition_from_list(&list, "disk12").expect("partition inventory"),
            Some(PathBuf::from("/dev/disk12s2"))
        );
        assert!(windows_partition_from_list(&list, "disk1;rm").is_err());
        assert!(validate_hdi_device(Path::new("/dev/disk8s2")).is_ok());
        assert!(validate_hdi_device(Path::new("/tmp/disk8s2")).is_err());
        assert!(validate_hdi_device(Path::new("/dev/disk8s2;rm")).is_err());
    }

    #[test]
    fn partition_inventory_rejects_a_volume_that_escaped_the_reviewed_disk() {
        let list: DiskList = serde_json::from_str(
            r#"{
                "AllDisksAndPartitions": [{
                    "DeviceIdentifier":"disk12",
                    "Partitions":[{"DeviceIdentifier":"disk99s1","VolumeName":"WINDOWS"}]
                }]
            }"#,
        )
        .expect("diskutil list fixture");

        let error = windows_partition_from_list(&list, "disk12")
            .expect_err("cross-disk partition must be rejected");
        assert!(matches!(error, Error::StalePlan(_)));
    }

    #[test]
    fn hdiutil_inventory_selects_a_device_with_a_mount_point() {
        let attached: HdiutilAttach = serde_json::from_str(
            r#"{
                "system-entities": [
                    {"dev-entry":"/dev/disk8"},
                    {"dev-entry":"/dev/disk8s1","mount-point":"/Volumes/WINDOWS_11"}
                ]
            }"#,
        )
        .expect("hdiutil fixture");
        let mounted = mounted_image_from_attach(attached).expect("mounted image");

        assert_eq!(mounted.device, Path::new("/dev/disk8s1"));
        assert_eq!(mounted.mount_point, Path::new("/Volumes/WINDOWS_11"));
    }

    #[test]
    fn windows_source_is_preflighted_and_copied_before_post_write_verification() {
        let source = windows_fixture();
        let destination = tempfile::tempdir().expect("destination");
        let control = OperationControl::new();
        let payload = preflight_windows_source(
            source.path(),
            WindowsPayload::Wim,
            &WindowsExperienceOptions::default(),
            &control,
        )
        .expect("safe source");

        copy_windows_tree(
            source.path(),
            destination.path(),
            &payload,
            WindowsPayload::Wim,
            64,
            &control,
            &mut |_| {},
        )
        .expect("copy tree");

        verify_windows_tree(destination.path()).expect("verified written tree");
        assert_eq!(
            fs::read(destination.path().join("sources/install.wim")).expect("payload"),
            b"install wim"
        );
    }

    #[test]
    fn windows_preflight_refuses_to_replace_an_answer_file() {
        let source = windows_fixture();
        fs::write(source.path().join("AutoUnattend.XML"), b"existing").expect("answer file");
        let options = WindowsExperienceOptions {
            bypass_hardware_requirements: true,
            ..WindowsExperienceOptions::default()
        };

        let error = preflight_windows_source(
            source.path(),
            WindowsPayload::Wim,
            &options,
            &OperationControl::new(),
        )
        .expect_err("must not replace answer file");

        assert!(matches!(error, Error::UnsupportedImage(_)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_diskutil_inventory_matches_the_guarded_schema() {
        let output = Command::new(DISKUTIL)
            .args(["list", "-plist"])
            .output()
            .expect("diskutil list");
        assert!(output.status.success(), "diskutil list failed");
        let list: DiskList = plist_json(&output.stdout, "diskutil list").expect("disk list plist");
        assert!(!list.all_disks_and_partitions.is_empty());
        for disk in list.all_disks_and_partitions {
            let identifier = disk.device_identifier.expect("whole disk identifier");
            validate_whole_disk_identifier(&identifier).expect("guarded whole disk identifier");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_hdiutil_attach_schema_round_trips_a_temporary_image() {
        let workspace = tempfile::tempdir().expect("workspace");
        let image = workspace.path().join("fixture.dmg");
        let output = Command::new(HDIUTIL)
            .args(["create", "-size", "8m", "-fs", "MS-DOS", "-volname"])
            .arg("BOOTABLE_TEST")
            .arg("-ov")
            .arg(&image)
            .output()
            .expect("hdiutil create");
        assert!(
            output.status.success(),
            "hdiutil create failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let mounted = attach_iso(&image).expect("attach temporary image");
        assert!(mounted.mount_point.starts_with("/Volumes"));
        detach_iso(&mounted.device).expect("detach temporary image");
    }

    #[test]
    fn io_registry_inventory_excludes_root_and_unstable_media() {
        let media: Vec<IoMedia> = serde_json::from_str(
            r#"[
                {"IORegistryEntryID":41,"BSD Name":"disk4","Whole":true,"Removable":true,"Ejectable":true,"Writable":true,"Size":64000000000,"Device Characteristics":{"Product Name":"USB disk","Serial Number":"SERIAL4","Vendor Name":"Example"},"Protocol Characteristics":{"Physical Interconnect":"USB"}},
                {"IORegistryEntryID":42,"BSD Name":"disk0","Whole":true,"Removable":true,"Ejectable":true,"Writable":true,"Size":1000,"Device Characteristics":{},"Protocol Characteristics":{"Physical Interconnect":"USB"}},
                {"BSD Name":"disk5","Whole":true,"Removable":true,"Ejectable":true,"Writable":true,"Size":1000,"Device Characteristics":{},"Protocol Characteristics":{"Physical Interconnect":"USB"}}
            ]"#,
        )
        .expect("ioreg fixture");
        let devices = devices_from_media(media, Some("disk0"));

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].path, Path::new("/dev/rdisk4"));
        assert_eq!(devices[0].id.as_str(), "serial:SERIAL4");
        assert!(devices[0].is_eligible_target());
    }
}
