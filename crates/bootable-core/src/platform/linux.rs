use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result, io_error};
use crate::model::{
    Device, DeviceId, MountPoint, Progress, ProgressPhase, WindowsExperienceOptions,
    WindowsPartitionScheme, WindowsPayload, WritePlan, WriteStrategy,
};
use crate::operation::OperationControl;
use crate::windows;
use serde::Deserialize;

const BUFFER_SIZE: usize = 4 * 1024 * 1024;
const FAT32_MAX_FILE_SIZE: u64 = u32::MAX as u64;
const WIM_CHUNK_MIB: &str = "3800";
const BASIC_DATA_GUID: &str = "0700";

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
        control.checkpoint()?;
        unmount_all(&target)?;
        progress(Progress {
            phase: ProgressPhase::Preparing,
            completed: 0,
            total: None,
            message: format!("Target identity verified: {}", target.display_name()),
        });
        check_bad_blocks(&target, plan.options.bad_block_check, control, progress)?;

        match plan.strategy {
            WriteStrategy::RawVerified => raw_write(plan, &target, control, progress),
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
        backup_device(&source, destination, progress)
    }
}

fn backup_device(
    source: &Device,
    destination: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    if !source.removable || source.system_disk {
        return Err(Error::UnsafeTarget(format!(
            "{} is not safe removable source media",
            source.path.display()
        )));
    }
    let extension = destination
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "img" | "raw" | "dd") {
        return Err(Error::UnsupportedImage(
            "drive backup currently writes raw .img, .raw, or .dd images".into(),
        ));
    }
    if destination.exists() {
        return Err(io_error(
            destination,
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "destination already exists",
            ),
        ));
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let destination_parent = parent
        .canonicalize()
        .map_err(|error| io_error(parent, error))?;
    if source.mounts.iter().any(|mount| {
        mount
            .path
            .canonicalize()
            .is_ok_and(|path| destination_parent.starts_with(path))
    }) {
        return Err(Error::UnsafeTarget(
            "the backup destination cannot be located on the source drive".into(),
        ));
    }

    unmount_all(source)?;
    progress(Progress {
        phase: ProgressPhase::Preparing,
        completed: 0,
        total: Some(source.capacity),
        message: format!("Source identity verified: {}", source.display_name()),
    });
    let mut input = BufReader::with_capacity(
        BUFFER_SIZE,
        File::open(&source.path).map_err(|error| io_error(&source.path, error))?,
    );
    let mut output = tempfile::NamedTempFile::new_in(&destination_parent)
        .map_err(|error| io_error(&destination_parent, error))?;
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    let mut copied = 0_u64;
    while copied < source.capacity {
        let requested = usize::try_from((source.capacity - copied).min(BUFFER_SIZE as u64))
            .map_err(|_| Error::UnsupportedImage("device size is not addressable".into()))?;
        let count = input
            .read(&mut buffer[..requested])
            .map_err(|error| io_error(&source.path, error))?;
        if count == 0 {
            return Err(io_error(
                &source.path,
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "device ended before its reported capacity",
                ),
            ));
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| io_error(destination, error))?;
        copied = copied.saturating_add(count as u64);
        progress(Progress {
            phase: ProgressPhase::Reading,
            completed: copied,
            total: Some(source.capacity),
            message: "Creating raw drive image".into(),
        });
    }
    output
        .as_file_mut()
        .sync_all()
        .map_err(|error| io_error(destination, error))?;
    output
        .persist_noclobber(destination)
        .map_err(|error| io_error(destination, error.error))?;
    progress(Progress {
        phase: ProgressPhase::Finished,
        completed: source.capacity,
        total: Some(source.capacity),
        message: format!("Drive image saved to {}", destination.display()),
    });
    Ok(())
}

fn check_bad_blocks(
    target: &Device,
    mode: crate::model::BadBlockCheck,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    const PATTERNS: [&str; 4] = ["0xaa", "0x55", "0xff", "0x00"];
    let passes = mode.passes();
    if passes == 0 {
        return Ok(());
    }
    ensure_tool("badblocks")?;
    for (index, pattern) in PATTERNS.iter().take(passes).enumerate() {
        control.checkpoint()?;
        progress(Progress {
            phase: ProgressPhase::Preparing,
            completed: index as u64,
            total: Some(passes as u64),
            message: format!(
                "Bad-block write/read test {}/{} with pattern {pattern}",
                index + 1,
                passes
            ),
        });
        run_status_controlled(
            "badblocks",
            [
                OsStr::new("-wsv"),
                OsStr::new("-b"),
                OsStr::new("4096"),
                OsStr::new("-t"),
                OsStr::new(pattern),
                target.path.as_os_str(),
            ],
            control,
        )?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct LsblkOutput {
    blockdevices: Vec<LsblkNode>,
}

#[derive(Debug, Deserialize)]
struct LsblkNode {
    path: PathBuf,
    #[serde(rename = "type")]
    kind: String,
    size: u64,
    rm: bool,
    ro: bool,
    tran: Option<String>,
    vendor: Option<String>,
    model: Option<String>,
    serial: Option<String>,
    mountpoints: Vec<Option<PathBuf>>,
    #[serde(default)]
    children: Vec<LsblkNode>,
}

fn discover_devices() -> Result<Vec<Device>> {
    let output = run_output(
        "lsblk",
        [
            "--json",
            "--tree",
            "--bytes",
            "--paths",
            "--output",
            "PATH,TYPE,SIZE,RM,RO,TRAN,VENDOR,MODEL,SERIAL,MOUNTPOINTS",
        ],
    )?;
    parse_devices(&output.stdout)
}

fn parse_devices(json: &[u8]) -> Result<Vec<Device>> {
    let parsed: LsblkOutput =
        serde_json::from_slice(json).map_err(|error| Error::InvalidToolOutput {
            program: "lsblk".into(),
            message: error.to_string(),
        })?;
    Ok(parsed
        .blockdevices
        .into_iter()
        .filter(|node| node.kind == "disk")
        .map(device_from_node)
        .filter(|device| device.removable)
        .collect())
}

fn device_from_node(node: LsblkNode) -> Device {
    let mounts = collect_mounts(&node);
    let system_disk = mounts.iter().any(|mount| mount.path == Path::new("/"));
    let removable = node.rm || node.tran.as_deref() == Some("usb");
    let serial = clean(node.serial);
    let id = match serial.as_deref() {
        Some(value) => DeviceId::new(format!("serial:{value}")),
        None => DeviceId::new(format!("path:{}:size:{}", node.path.display(), node.size)),
    };
    Device {
        id,
        path: node.path,
        vendor: clean(node.vendor),
        model: clean(node.model),
        serial,
        transport: clean(node.tran),
        capacity: node.size,
        removable,
        read_only: node.ro,
        system_disk,
        mounts,
    }
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn collect_mounts(node: &LsblkNode) -> Vec<MountPoint> {
    let mut mounts = node
        .mountpoints
        .iter()
        .flatten()
        .map(|path| MountPoint {
            device: node.path.clone(),
            path: path.clone(),
        })
        .collect::<Vec<_>>();
    for child in &node.children {
        mounts.extend(collect_mounts(child));
    }
    mounts
}

fn refresh_target(plan: &WritePlan) -> Result<Device> {
    let target = discover_devices()?
        .into_iter()
        .find(|device| device.id == plan.target.id)
        .ok_or_else(|| Error::DeviceNotFound(plan.target.id.to_string()))?;
    if target.capacity != plan.target.capacity {
        return Err(Error::StalePlan(format!(
            "capacity changed from {} to {} bytes",
            plan.target.capacity, target.capacity
        )));
    }
    if !target.removable || target.system_disk || target.read_only {
        return Err(Error::StalePlan(
            "the refreshed device no longer passes the safety checks".into(),
        ));
    }
    Ok(target)
}

fn unmount_all(target: &Device) -> Result<()> {
    for mount in target.mounts.iter().rev() {
        run_status("umount", [mount.path.as_os_str()])?;
    }
    Ok(())
}

fn raw_write(
    plan: &WritePlan,
    target: &Device,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    super::raw::write(&plan.image, &target.path, control, progress)
}

fn windows_write(
    plan: &WritePlan,
    target: &Device,
    payload: WindowsPayload,
    partition_scheme: WindowsPartitionScheme,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    for tool in [
        "wipefs",
        "partprobe",
        "mkfs.fat",
        "mount",
        "umount",
        "findmnt",
        "sync",
    ] {
        ensure_tool(tool)?;
    }
    ensure_tool(match partition_scheme {
        WindowsPartitionScheme::Gpt => "sgdisk",
        WindowsPartitionScheme::Mbr => "parted",
    })?;

    let workspace = tempfile::tempdir().map_err(|error| io_error("temporary directory", error))?;
    let iso_mount = workspace.path().join("iso");
    let usb_mount = workspace.path().join("usb");
    fs::create_dir_all(&iso_mount).map_err(|error| io_error(&iso_mount, error))?;
    fs::create_dir_all(&usb_mount).map_err(|error| io_error(&usb_mount, error))?;

    run_status(
        "mount",
        [
            OsStr::new("-o"),
            OsStr::new("loop,ro"),
            plan.image.path.as_os_str(),
            iso_mount.as_os_str(),
        ],
    )?;

    let result = (|| {
        control.checkpoint()?;
        progress(Progress {
            phase: ProgressPhase::Preparing,
            completed: 0,
            total: Some(plan.image.size),
            message: format!("Creating {partition_scheme} Windows media partition"),
        });
        // Hybrid ISO images can make `sgdisk --zap-all` return an error even
        // after it has erased the old table. Clear every known signature first,
        // then ask sgdisk to initialize a fresh GPT deterministically.
        run_status(
            "wipefs",
            [
                OsStr::new("--all"),
                OsStr::new("--force"),
                target.path.as_os_str(),
            ],
        )?;
        match partition_scheme {
            WindowsPartitionScheme::Gpt => {
                run_status("sgdisk", [OsStr::new("--clear"), target.path.as_os_str()])?;
                run_status(
                    "sgdisk",
                    [
                        OsStr::new("--new=1:0:0"),
                        OsStr::new("--typecode=1:0700"),
                        OsStr::new("--change-name=1:BOOTABLE"),
                        target.path.as_os_str(),
                    ],
                )?;
            }
            WindowsPartitionScheme::Mbr => run_status(
                "parted",
                [
                    OsStr::new("--script"),
                    target.path.as_os_str(),
                    OsStr::new("mklabel"),
                    OsStr::new("msdos"),
                    OsStr::new("mkpart"),
                    OsStr::new("primary"),
                    OsStr::new("fat32"),
                    OsStr::new("1MiB"),
                    OsStr::new("100%"),
                    OsStr::new("set"),
                    OsStr::new("1"),
                    OsStr::new("boot"),
                    OsStr::new("on"),
                ],
            )?,
        }
        run_status("partprobe", [target.path.as_os_str()])?;
        let partition = wait_for_partition(&target.path)?;
        run_status(
            "mkfs.fat",
            [
                OsStr::new("-F"),
                OsStr::new("32"),
                OsStr::new("-n"),
                OsStr::new("WINDOWS"),
                partition.as_os_str(),
            ],
        )?;
        unmount_device_if_mounted(&partition)?;
        run_status("mount", [partition.as_os_str(), usb_mount.as_os_str()])?;

        let copy_result = copy_windows_tree(
            &iso_mount,
            &usb_mount,
            payload,
            plan.image.size,
            control,
            progress,
        )
        .and_then(|()| apply_windows_options(&usb_mount, &plan.options.windows))
        .and_then(|()| {
            if plan.options.windows.use_windows_ca_2023 {
                apply_windows_ca_2023(&usb_mount, control)
            } else {
                Ok(())
            }
        });
        let sync_result = if copy_result.is_ok() {
            run_status("sync", [OsStr::new("-f"), usb_mount.as_os_str()])
        } else {
            Ok(())
        };
        let unmount_result = run_status("umount", [usb_mount.as_os_str()]);
        copy_result?;
        sync_result?;
        unmount_result?;
        Ok(())
    })();

    let iso_unmount_result = run_status("umount", [iso_mount.as_os_str()]);
    result?;
    iso_unmount_result?;
    progress(Progress {
        phase: ProgressPhase::Finished,
        completed: plan.image.size,
        total: Some(plan.image.size),
        message: match partition_scheme {
            WindowsPartitionScheme::Gpt => {
                format!("Windows installer ready on FAT32 (partition type {BASIC_DATA_GUID})")
            }
            WindowsPartitionScheme::Mbr => {
                "Windows installer ready on active MBR FAT32 media for UEFI systems".into()
            }
        },
    });
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
    let answer = windows::answer_file(options)?;
    fs::write(&path, answer).map_err(|error| io_error(path, error))
}

fn apply_windows_ca_2023(root: &Path, control: &OperationControl) -> Result<()> {
    ensure_tool("wimlib-imagex")?;
    let boot_wim = root.join("sources/boot.wim");
    if !boot_wim.is_file() {
        return Err(Error::UnsupportedImage(
            "Windows CA 2023 media requires sources/boot.wim".into(),
        ));
    }
    let extracted = tempfile::tempdir().map_err(|error| io_error("temporary directory", error))?;
    let destination = format!("--dest-dir={}", extracted.path().display());
    run_status_controlled(
        "wimlib-imagex",
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
    let boot_directory = root.join("efi/boot");
    let fallback = fs::read_dir(&boot_directory)
        .map_err(|error| io_error(&boot_directory, error))?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.starts_with("boot") && name.ends_with(".efi"))
        })
        .ok_or_else(|| {
            Error::UnsupportedImage("Windows media has no UEFI fallback bootloader".into())
        })?;
    fs::copy(&boot_manager, &fallback).map_err(|error| io_error(&fallback, error))?;
    let root_boot_manager = root.join("bootmgr.efi");
    fs::copy(&root_manager, &root_boot_manager)
        .map_err(|error| io_error(&root_boot_manager, error))?;
    copy_ca_2023_fonts(&fonts_ex, &root.join("efi/microsoft/boot/Fonts"), &fonts_ex)?;
    Ok(())
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
    payload: WindowsPayload,
    total: u64,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    control.checkpoint()?;
    let payload_path = find_install_payload(source, payload)?;
    let payload_size = payload_path
        .metadata()
        .map_err(|error| io_error(&payload_path, error))?
        .len();
    let split_payload = payload_size > FAT32_MAX_FILE_SIZE;
    if split_payload {
        ensure_tool("wimlib-imagex")?;
    }

    let mut copied = 0_u64;
    copy_tree(
        source,
        destination,
        Some(&payload_path),
        &mut copied,
        total,
        control,
        progress,
    )?;
    if split_payload {
        let split_target = destination.join("sources/install.swm");
        run_status_controlled(
            "wimlib-imagex",
            [
                OsStr::new("split"),
                payload_path.as_os_str(),
                split_target.as_os_str(),
                OsStr::new(WIM_CHUNK_MIB),
            ],
            control,
        )?;
        control.checkpoint()?;
        progress(Progress {
            phase: ProgressPhase::Verifying,
            completed: total,
            total: Some(total),
            message: "Verifying the complete split-WIM set".into(),
        });
        verify_split_wim(destination, control)?;
    } else {
        let relative = payload_path
            .strip_prefix(source)
            .map_err(|_| Error::UnsupportedImage("payload escaped the mounted image".into()))?;
        copy_file(
            &payload_path,
            &destination.join(relative),
            &mut copied,
            total,
            control,
            progress,
        )?;
    }

    verify_windows_tree(destination)?;
    Ok(())
}

fn verify_split_wim(root: &Path, control: &OperationControl) -> Result<()> {
    let sources = find_case_insensitive_child(root, "sources")?;
    let first_part = find_case_insensitive_child(&sources, "install.swm")?;
    let mut reference = OsString::from("--ref=");
    reference.push(sources.join("install*.swm"));
    run_status_controlled(
        "wimlib-imagex",
        [
            OsStr::new("verify"),
            first_part.as_os_str(),
            reference.as_os_str(),
        ],
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
    writer
        .flush()
        .map_err(|error| io_error(destination, error))?;
    Ok(())
}

fn find_install_payload(root: &Path, payload: WindowsPayload) -> Result<PathBuf> {
    let sources = find_case_insensitive_child(root, "sources")?;
    let prefix = match payload {
        WindowsPayload::Wim => "install.wim",
        WindowsPayload::Esd => "install.esd",
        WindowsPayload::SplitWim => "install.swm",
    };
    find_case_insensitive_child(&sources, prefix)
}

fn find_case_insensitive_child(parent: &Path, name: &str) -> Result<PathBuf> {
    fs::read_dir(parent)
        .map_err(|error| io_error(parent, error))?
        .filter_map(std::result::Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
        })
        .map(|entry| entry.path())
        .ok_or_else(|| {
            Error::UnsupportedImage(format!("missing {name} below {}", parent.display()))
        })
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
    verify_fat_file_sizes(root)?;
    Ok(())
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

fn wait_for_partition(device: &Path) -> Result<PathBuf> {
    let partition = partition_path(device);
    for _ in 0..25 {
        if partition.exists() {
            return Ok(partition);
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(Error::DeviceNotFound(partition.display().to_string()))
}

fn unmount_device_if_mounted(device: &Path) -> Result<()> {
    let output = Command::new("findmnt")
        .args(["--noheadings", "--output", "TARGET", "--source"])
        .arg(device)
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::MissingTool("findmnt")
            } else {
                io_error("findmnt", error)
            }
        })?;
    if output.status.success() {
        for mount in String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|mount| !mount.is_empty())
        {
            run_status("umount", [OsStr::new(mount)])?;
        }
        return Ok(());
    }
    if output.status.code() == Some(1) {
        return Ok(());
    }
    Err(Error::CommandFailed {
        program: "findmnt".into(),
        message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

fn partition_path(device: &Path) -> PathBuf {
    let device_text = device.to_string_lossy();
    let separator = if device_text
        .chars()
        .last()
        .is_some_and(|character| character.is_ascii_digit())
    {
        "p"
    } else {
        ""
    };
    PathBuf::from(format!("{device_text}{separator}1"))
}

fn is_root() -> Result<bool> {
    let status = fs::read_to_string("/proc/self/status")
        .map_err(|error| io_error("/proc/self/status", error))?;
    let effective = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|ids| ids.split_whitespace().nth(1))
        .and_then(|id| id.parse::<u32>().ok())
        .ok_or_else(|| Error::InvalidToolOutput {
            program: "/proc/self/status".into(),
            message: "missing effective user ID".into(),
        })?;
    Ok(effective == 0)
}

fn ensure_tool(program: &'static str) -> Result<()> {
    Command::new(program)
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|_| ())
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::MissingTool(program)
            } else {
                io_error(program, error)
            }
        })
}

fn run_output<I, S>(program: &'static str, arguments: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::MissingTool(program)
            } else {
                io_error(program, error)
            }
        })?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            program: program.into(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(output)
}

fn run_status<I, S>(program: &'static str, arguments: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = run_output(program, arguments)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::CommandFailed {
            program: program.into(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

fn run_status_controlled<I, S>(
    program: &'static str,
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::MissingTool(program)
            } else {
                io_error(program, error)
            }
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::InvalidToolOutput {
            program: program.into(),
            message: "command did not provide stdout".into(),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::InvalidToolOutput {
            program: program.into(),
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
        match child.try_wait().map_err(|error| io_error(program, error))? {
            Some(status) => break status,
            None => thread::sleep(Duration::from_millis(50)),
        }
    };
    let _stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(Error::CommandFailed {
            program: program.into(),
            message: String::from_utf8_lossy(&stderr).trim().to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Seek, SeekFrom};
    use std::time::Instant;

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::model::{CompressedImageKind, ImageCompression, ImageKind, ImageReport};

    #[test]
    fn partition_names_handle_sd_and_nvme_devices() {
        assert_eq!(
            partition_path(Path::new("/dev/sda")),
            Path::new("/dev/sda1")
        );
        assert_eq!(
            partition_path(Path::new("/dev/nvme0n1")),
            Path::new("/dev/nvme0n1p1")
        );
    }

    #[test]
    fn root_detection_uses_the_effective_uid() {
        assert!(is_root().is_ok());
    }

    #[test]
    fn nested_root_mount_marks_the_whole_disk_as_system() {
        let json = br#"{
            "blockdevices": [{
                "path": "/dev/nvme0n1",
                "type": "disk",
                "size": 1000000,
                "rm": true,
                "ro": false,
                "tran": "usb",
                "vendor": null,
                "model": "Test disk",
                "serial": "SYSTEM1",
                "mountpoints": [],
                "children": [{
                    "path": "/dev/nvme0n1p1",
                    "type": "part",
                    "size": 900000,
                    "rm": false,
                    "ro": false,
                    "tran": "nvme",
                    "vendor": null,
                    "model": null,
                    "serial": null,
                    "mountpoints": ["/"]
                }]
            }]
        }"#;

        let devices = parse_devices(json).expect("valid lsblk fixture");

        assert_eq!(devices.len(), 1);
        assert!(devices[0].removable);
        assert!(devices[0].system_disk);
    }

    #[test]
    fn discovery_hides_non_removable_disks() {
        let json = br#"{
            "blockdevices": [
                {
                    "path": "/dev/nvme0n1",
                    "type": "disk",
                    "size": 1000000,
                    "rm": false,
                    "ro": false,
                    "tran": "nvme",
                    "vendor": null,
                    "model": "Internal disk",
                    "serial": "INTERNAL1",
                    "mountpoints": []
                },
                {
                    "path": "/dev/sdb",
                    "type": "disk",
                    "size": 128000000000,
                    "rm": true,
                    "ro": false,
                    "tran": "usb",
                    "vendor": "SanDisk",
                    "model": "USB stick",
                    "serial": "REMOVABLE1",
                    "mountpoints": []
                }
            ]
        }"#;

        let devices = parse_devices(json).expect("valid lsblk fixture");

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].path, Path::new("/dev/sdb"));
        assert!(devices[0].removable);
    }

    #[test]
    fn sha_verification_accepts_identical_prefixes() {
        let expected = b"bootable";
        let mut target = tempfile::NamedTempFile::new().expect("target");
        target
            .write_all(b"bootable-trailing")
            .expect("target write");
        target.as_file_mut().seek(SeekFrom::Start(0)).expect("seek");
        let expected_hash: [u8; 32] = Sha256::digest(expected).into();

        crate::platform::raw::verify_target_hash(
            target.path(),
            expected.len() as u64,
            expected_hash,
            &OperationControl::new(),
            &mut |_| {},
        )
        .expect("verify");
    }

    #[test]
    fn verification_honors_cancellation_before_reading_media() {
        let target = tempfile::NamedTempFile::new().expect("target");
        let control = OperationControl::new();
        control.cancel();

        let error = crate::platform::raw::verify_target_hash(
            target.path(),
            1,
            [0; 32],
            &control,
            &mut |_| {},
        )
        .expect_err("cancelled verification");
        assert!(matches!(error, Error::OperationCancelled));
    }

    #[test]
    fn controlled_commands_are_terminated_after_cancellation() {
        let control = OperationControl::new();
        let worker_control = control.clone();
        let started = Instant::now();
        let worker = thread::spawn(move || run_status_controlled("sleep", ["5"], &worker_control));
        thread::sleep(Duration::from_millis(100));
        control.cancel();

        let error = worker
            .join()
            .expect("worker")
            .expect_err("cancelled command");
        assert!(matches!(error, Error::OperationCancelled));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn compressed_raw_images_stream_and_verify_without_staging() {
        let payload = (0..(2 * 1024 * 1024 + 17))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let source = tempfile::Builder::new()
            .suffix(".img.xz")
            .tempfile()
            .expect("source");
        let mut encoder = xz2::write::XzEncoder::new(source.reopen().expect("source reopen"), 1);
        encoder.write_all(&payload).expect("compress payload");
        encoder.finish().expect("finish compression");
        let target_file = tempfile::NamedTempFile::new().expect("target");
        target_file
            .as_file()
            .set_len(payload.len() as u64 + 4096)
            .expect("target capacity");
        let target = Device {
            id: DeviceId::new("test:compressed-target"),
            path: target_file.path().to_path_buf(),
            vendor: Some("Bootable".into()),
            model: Some("integration fixture".into()),
            serial: Some("COMPRESSED1".into()),
            transport: Some("test".into()),
            capacity: payload.len() as u64 + 4096,
            removable: true,
            read_only: false,
            system_disk: false,
            mounts: Vec::new(),
        };
        let plan = crate::plan::build(
            ImageReport {
                path: source.path().to_path_buf(),
                size: payload.len() as u64,
                kind: ImageKind::CompressedDiskImage {
                    compression: ImageCompression::Xz,
                    inner: CompressedImageKind::RawDiskImage,
                },
                volume_label: None,
                warnings: Vec::new(),
            },
            target.clone(),
        )
        .expect("plan");

        raw_write(&plan, &target, &OperationControl::new(), &mut |_| {}).expect("write and verify");

        let mut written = vec![0_u8; payload.len()];
        File::open(target_file.path())
            .expect("open target")
            .read_exact(&mut written)
            .expect("read target");
        assert_eq!(written, payload);
    }

    #[test]
    #[ignore = "requires a root-created temporary loop device from scripts/loop-write-smoke.sh"]
    fn temporary_loop_device_streams_and_verifies_without_relaxing_discovery() {
        use std::os::unix::fs::FileTypeExt;

        let source_path = std::env::var_os("BOOTABLE_LOOP_SOURCE")
            .map(PathBuf::from)
            .expect("BOOTABLE_LOOP_SOURCE is set by the harness");
        let device_path = std::env::var_os("BOOTABLE_LOOP_DEVICE")
            .map(PathBuf::from)
            .expect("BOOTABLE_LOOP_DEVICE is set by the harness");
        assert!(
            device_path
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| {
                    name.strip_prefix("loop")
                        .is_some_and(|suffix| suffix.chars().all(|value| value.is_ascii_digit()))
                }),
            "the destructive fixture accepts only /dev/loopN"
        );
        assert!(
            device_path
                .metadata()
                .expect("loop metadata")
                .file_type()
                .is_block_device(),
            "fixture target must be a block device"
        );
        let source_size = source_path.metadata().expect("source metadata").len();
        let capacity_output = run_output(
            "blockdev",
            [
                "--getsize64",
                device_path.to_str().expect("UTF-8 loop path"),
            ],
        )
        .expect("read loop capacity");
        let capacity = String::from_utf8_lossy(&capacity_output.stdout)
            .trim()
            .parse::<u64>()
            .expect("numeric loop capacity");
        assert!(source_size > 0 && source_size < capacity);
        let target = Device {
            id: DeviceId::new(format!("loop-fixture:{}", device_path.display())),
            path: device_path,
            vendor: Some("Bootable".into()),
            model: Some("temporary loop fixture".into()),
            serial: Some("LOOPFIXTURE".into()),
            transport: Some("loop-test".into()),
            capacity,
            removable: true,
            read_only: false,
            system_disk: false,
            mounts: Vec::new(),
        };
        let plan = crate::plan::build(
            ImageReport {
                path: source_path,
                size: source_size,
                kind: ImageKind::RawDiskImage,
                volume_label: None,
                warnings: Vec::new(),
            },
            target.clone(),
        )
        .expect("fixture plan");

        raw_write(&plan, &target, &OperationControl::new(), &mut |_| {})
            .expect("loop write and verification");
    }

    #[test]
    fn raw_backup_is_atomic_and_refuses_overwrite() {
        let expected = b"bootable-backup";
        let source_file = tempfile::NamedTempFile::new().expect("source");
        fs::write(source_file.path(), expected).expect("source contents");
        let directory = tempfile::tempdir().expect("destination directory");
        let destination = directory.path().join("usb.img");
        let source = Device {
            id: DeviceId::new("serial:BACKUP1"),
            path: source_file.path().to_path_buf(),
            vendor: Some("Test".into()),
            model: Some("Drive".into()),
            serial: Some("BACKUP1".into()),
            transport: Some("usb".into()),
            capacity: expected.len() as u64,
            removable: true,
            read_only: false,
            system_disk: false,
            mounts: Vec::new(),
        };

        backup_device(&source, &destination, &mut |_| {}).expect("backup");

        assert_eq!(fs::read(&destination).expect("backup contents"), expected);
        let error = backup_device(&source, &destination, &mut |_| {}).expect_err("no overwrite");
        assert!(matches!(error, Error::Io { .. }));
    }

    #[test]
    fn windows_tree_verification_requires_boot_and_install_files() {
        let media = tempfile::tempdir().expect("media directory");
        for directory in ["efi/boot", "sources"] {
            fs::create_dir_all(media.path().join(directory)).expect("create directory");
        }
        for file in [
            "efi/boot/bootx64.efi",
            "bootmgr",
            "sources/boot.wim",
            "sources/install.swm",
        ] {
            File::create(media.path().join(file)).expect("create fixture file");
        }

        verify_windows_tree(media.path()).expect("valid Windows tree");

        fs::remove_file(media.path().join("sources/install.swm")).expect("remove payload");
        assert!(verify_windows_tree(media.path()).is_err());
    }

    #[test]
    fn windows_requirement_bypass_is_written_without_overwriting() {
        let media = tempfile::tempdir().expect("media directory");
        let options = WindowsExperienceOptions {
            bypass_hardware_requirements: true,
            ..WindowsExperienceOptions::default()
        };

        apply_windows_options(media.path(), &options).expect("write answer file");
        let answer_file =
            fs::read_to_string(media.path().join("autounattend.xml")).expect("read answer file");
        for value in ["BypassTPMCheck", "BypassSecureBootCheck", "BypassRAMCheck"] {
            assert!(answer_file.contains(value));
        }

        let error = apply_windows_options(media.path(), &options).expect_err("refuse overwrite");
        assert!(matches!(error, Error::UnsupportedImage(_)));
    }

    #[test]
    fn windows_experience_options_share_one_guarded_answer_file() {
        let options = WindowsExperienceOptions {
            bypass_hardware_requirements: true,
            allow_offline_account: true,
            minimize_data_collection: true,
            disable_bitlocker: true,
            ..WindowsExperienceOptions::default()
        };

        let answer_file = windows::answer_file(&options).expect("answer file");

        for value in [
            "BypassTPMCheck",
            "HideOnlineAccountScreens",
            "ProtectYourPC",
            "PreventDeviceEncryption",
        ] {
            assert!(answer_file.contains(value), "missing {value}");
        }
        assert_eq!(answer_file.matches("<unattend ").count(), 1);
    }

    #[test]
    fn ca_2023_fonts_are_copied_without_ex_suffixes() {
        let source = tempfile::tempdir().expect("source");
        let destination = tempfile::tempdir().expect("destination");
        let nested = source.path().join("sub_EX");
        fs::create_dir_all(&nested).expect("nested source");
        fs::write(nested.join("wgl4_boot_EX.ttf"), b"font").expect("font fixture");

        copy_ca_2023_fonts(source.path(), destination.path(), source.path()).expect("copy fonts");

        assert_eq!(
            fs::read(destination.path().join("sub/wgl4_boot.ttf")).expect("copied font"),
            b"font"
        );
    }
}
