use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
use crate::windows_media::{
    FAT32_MAX_FILE_SIZE, apply_setup_options as apply_windows_options, find_case_insensitive_child,
    find_install_payload, find_optional_case_insensitive_child,
    reject_oversized_files_except as reject_other_oversized_files,
    verify_written_tree as verify_windows_tree,
};

const BUFFER_SIZE: usize = 4 * 1024 * 1024;
const WINDOWS_FORMAT_LIMIT: u64 = 32 * 1024 * 1024 * 1024 - 1024 * 1024;
const WINDOWS_FREE_SPACE_ALLOWANCE: u64 = 256 * 1024 * 1024;

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
        if !is_administrator()? {
            return Err(Error::NotPrivileged);
        }
        if plan.options.bad_block_check.passes() > 0 {
            return Err(Error::PlatformUnavailable(
                "the destructive bad-block test is currently available only on Linux".into(),
            ));
        }
        let target = refresh_target(plan)?;
        let number = physical_drive_number(&target.path)?;
        detach_drive_letters(number)?;
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
        if !is_administrator()? {
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

fn windows_write(
    plan: &WritePlan,
    target: &Device,
    payload: WindowsPayload,
    partition_scheme: WindowsPartitionScheme,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    if plan.image.size.saturating_add(WINDOWS_FREE_SPACE_ALLOWANCE) > WINDOWS_FORMAT_LIMIT {
        return Err(Error::UnsupportedImage(
            "the Windows installer is too large for Windows' native FAT32 formatter; use Bootable on Linux for this image"
                .into(),
        ));
    }
    let source = mount_iso(&plan.image.path, control)?;
    let result = (|| {
        let payload_path = preflight_windows_source(&source, payload, &plan.options.windows)?;
        if plan.options.windows.use_windows_ca_2023 {
            preflight_ca_2023(&source, control)?;
        }
        control.checkpoint()?;
        progress(Progress {
            phase: ProgressPhase::Preparing,
            completed: 0,
            total: Some(plan.image.size),
            message: format!("Creating {partition_scheme} Windows FAT32 media"),
        });
        let number = physical_drive_number(&target.path)?;
        let destination = prepare_fat32_partition(number, partition_scheme, control)?;
        copy_windows_tree(
            &source,
            &destination,
            &payload_path,
            payload,
            plan.image.size,
            control,
            progress,
        )?;
        apply_windows_options(&destination, &plan.options.windows)?;
        if plan.options.windows.use_windows_ca_2023 {
            apply_windows_ca_2023(&source, &destination, control)?;
        }
        verify_windows_tree(&destination)?;
        progress(Progress {
            phase: ProgressPhase::Finished,
            completed: plan.image.size,
            total: Some(plan.image.size),
            message: format!("Windows installer ready on {partition_scheme} FAT32 media"),
        });
        Ok(())
    })();
    let unmount = dismount_iso(&plan.image.path);
    result?;
    unmount
}

fn mount_iso(image: &Path, control: &OperationControl) -> Result<PathBuf> {
    const SCRIPT: &str = "$ErrorActionPreference='Stop'; $disk=Mount-DiskImage -ImagePath $env:BOOTABLE_IMAGE -PassThru; $volume=$disk | Get-Volume | Where-Object {$_.DriveLetter} | Select-Object -First 1; if (-not $volume) { throw 'mounted ISO has no drive letter' }; [Console]::Out.Write($volume.DriveLetter)";
    let output = powershell_controlled(
        SCRIPT,
        [("BOOTABLE_IMAGE", image.to_string_lossy().as_ref())],
        control,
    )?;
    drive_root(output.trim())
}

fn dismount_iso(image: &Path) -> Result<()> {
    const SCRIPT: &str =
        "$ErrorActionPreference='Stop'; Dismount-DiskImage -ImagePath $env:BOOTABLE_IMAGE";
    powershell_with_environment(
        SCRIPT,
        [("BOOTABLE_IMAGE", image.to_string_lossy().as_ref())],
    )
    .map(|_| ())
}

fn prepare_fat32_partition(
    number: u32,
    partition_scheme: WindowsPartitionScheme,
    control: &OperationControl,
) -> Result<PathBuf> {
    const SCRIPT: &str = "$ErrorActionPreference='Stop'; $number=[uint32]$env:BOOTABLE_DISK_NUMBER; $style=$env:BOOTABLE_PARTITION_STYLE; Clear-Disk -Number $number -RemoveData -RemoveOEM -Confirm:$false; Initialize-Disk -Number $number -PartitionStyle $style -PassThru | Out-Null; $disk=Get-Disk -Number $number; $limit=[uint64](32GB-1MB); $available=[uint64]($disk.Size-16MB); $size=[Math]::Min($limit,$available); $partition=New-Partition -DiskNumber $number -Size $size -AssignDriveLetter; if ($style -eq 'MBR') { Set-Partition -DiskNumber $number -PartitionNumber $partition.PartitionNumber -IsActive $true }; Format-Volume -Partition $partition -FileSystem FAT32 -NewFileSystemLabel BOOTABLE -Confirm:$false -Force | Out-Null; [Console]::Out.Write($partition.DriveLetter)";
    let number = number.to_string();
    let style = match partition_scheme {
        WindowsPartitionScheme::Gpt => "GPT",
        WindowsPartitionScheme::Mbr => "MBR",
    };
    let output = powershell_controlled(
        SCRIPT,
        [
            ("BOOTABLE_DISK_NUMBER", number.as_str()),
            ("BOOTABLE_PARTITION_STYLE", style),
        ],
        control,
    )?;
    drive_root(output.trim())
}

fn drive_root(letter: &str) -> Result<PathBuf> {
    if letter.len() != 1 || !letter.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err(Error::InvalidToolOutput {
            program: "PowerShell volume discovery".into(),
            message: format!("unexpected drive letter `{letter}`"),
        });
    }
    Ok(PathBuf::from(format!("{}:\\", letter.to_ascii_uppercase())))
}

fn preflight_windows_source(
    source: &Path,
    payload: WindowsPayload,
    options: &WindowsExperienceOptions,
) -> Result<PathBuf> {
    if options.requires_answer_file() {
        windows::answer_file(options)?;
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
    let payload_path = find_install_payload(source, payload)?;
    let payload_size = payload_path
        .metadata()
        .map_err(|error| io_error(&payload_path, error))?
        .len();
    if payload == WindowsPayload::Esd && payload_size > FAT32_MAX_FILE_SIZE {
        return Err(Error::UnsupportedImage(
            "DISM cannot split an oversized install.esd; use Bootable on Linux for this image"
                .into(),
        ));
    }
    if options.requires_answer_file()
        && find_optional_case_insensitive_child(source, "autounattend.xml").is_some()
    {
        return Err(Error::UnsupportedImage(
            "the source already contains autounattend.xml; refusing to overwrite it".into(),
        ));
    }
    reject_other_oversized_files(source, &payload_path)?;
    Ok(payload_path)
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
        if payload != WindowsPayload::Wim {
            return Err(Error::UnsupportedImage(
                "only install.wim can be split by native Windows DISM".into(),
            ));
        }
        let sources = find_case_insensitive_child(destination, "sources")?;
        let split = sources.join("install.swm");
        let mut command = Command::new("dism.exe");
        command
            .arg("/English")
            .arg("/Split-Image")
            .arg(format!("/ImageFile:{}", payload_path.display()))
            .arg(format!("/SWMFile:{}", split.display()))
            .arg("/FileSize:3800");
        run_controlled(&mut command, control)?;
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
    writer.flush().map_err(|error| io_error(destination, error))
}

fn preflight_ca_2023(source: &Path, control: &OperationControl) -> Result<()> {
    let sources = find_case_insensitive_child(source, "sources")?;
    let boot_wim = find_case_insensitive_child(&sources, "boot.wim")?;
    let mut command = Command::new("dism.exe");
    command
        .arg("/English")
        .arg("/Get-WimInfo")
        .arg(format!("/WimFile:{}", boot_wim.display()))
        .arg("/Index:2");
    run_controlled(&mut command, control)
}

fn apply_windows_ca_2023(
    source: &Path,
    destination: &Path,
    control: &OperationControl,
) -> Result<()> {
    let sources = find_case_insensitive_child(source, "sources")?;
    let boot_wim = find_case_insensitive_child(&sources, "boot.wim")?;
    let workspace = tempfile::tempdir().map_err(|error| io_error("temporary directory", error))?;
    let mount = workspace.path().join("boot-wim");
    fs::create_dir(&mount).map_err(|error| io_error(&mount, error))?;
    let mut mount_command = Command::new("dism.exe");
    mount_command
        .arg("/English")
        .arg("/Mount-Image")
        .arg(format!("/ImageFile:{}", boot_wim.display()))
        .arg("/Index:2")
        .arg(format!("/MountDir:{}", mount.display()))
        .arg("/ReadOnly");
    run_controlled(&mut mount_command, control)?;

    let result = (|| {
        let efi_ex = mount.join("Windows/Boot/EFI_EX");
        let fonts_ex = mount.join("Windows/Boot/Fonts_EX");
        let boot_manager = find_case_insensitive_child(&efi_ex, "bootmgfw_EX.efi")?;
        let root_manager = find_case_insensitive_child(&efi_ex, "bootmgr_EX.efi")?;
        let efi = find_case_insensitive_child(destination, "efi")?;
        let boot_directory = find_case_insensitive_child(&efi, "boot")?;
        let fallback = fs::read_dir(&boot_directory)
            .map_err(|error| io_error(&boot_directory, error))?
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        let name = name.to_ascii_lowercase();
                        name.starts_with("boot") && name.ends_with(".efi")
                    })
            })
            .ok_or_else(|| {
                Error::UnsupportedImage("Windows media has no UEFI fallback bootloader".into())
            })?;
        fs::copy(boot_manager, &fallback).map_err(|error| io_error(&fallback, error))?;
        let root_boot_manager = destination.join("bootmgr.efi");
        fs::copy(root_manager, &root_boot_manager)
            .map_err(|error| io_error(&root_boot_manager, error))?;
        let microsoft = find_case_insensitive_child(&efi, "microsoft")?;
        let microsoft_boot = find_case_insensitive_child(&microsoft, "boot")?;
        let fonts = find_case_insensitive_child(&microsoft_boot, "fonts")?;
        copy_ca_fonts(&fonts_ex, &fonts, &fonts_ex)?;
        Ok(())
    })();

    let mut unmount = Command::new("dism.exe");
    unmount
        .arg("/English")
        .arg("/Unmount-Image")
        .arg(format!("/MountDir:{}", mount.display()))
        .arg("/Discard");
    let cleanup = run_controlled(&mut unmount, &OperationControl::new());
    result?;
    cleanup
}

fn copy_ca_fonts(source: &Path, destination: &Path, root: &Path) -> Result<()> {
    for entry in fs::read_dir(source).map_err(|error| io_error(source, error))? {
        let entry = entry.map_err(|error| io_error(source, error))?;
        let path = entry.path();
        if path.is_dir() {
            copy_ca_fonts(&path, destination, root)?;
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            Error::UnsupportedImage("CA 2023 font extraction escaped its workspace".into())
        })?;
        let target = destination.join(PathBuf::from(relative.to_string_lossy().replace("_EX", "")));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        fs::copy(&path, &target).map_err(|error| io_error(&target, error))?;
    }
    Ok(())
}

fn powershell_controlled<const N: usize>(
    script: &str,
    environment: [(&str, &str); N],
    control: &OperationControl,
) -> Result<String> {
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]);
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = run_controlled_output(&mut command, control)?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

fn powershell_with_environment<const N: usize>(
    script: &str,
    environment: [(&str, &str); N],
) -> Result<String> {
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]);
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = command
        .output()
        .map_err(|error| io_error("powershell.exe", error))?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            program: "powershell.exe".into(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_controlled(command: &mut Command, control: &OperationControl) -> Result<()> {
    run_controlled_output(command, control).map(|_| ())
}

fn run_controlled_output(command: &mut Command, control: &OperationControl) -> Result<Vec<u8>> {
    control.checkpoint()?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .spawn()
        .map_err(|error| match (error.kind(), program.as_str()) {
            (std::io::ErrorKind::NotFound, "dism.exe") => Error::MissingTool("dism.exe"),
            (std::io::ErrorKind::NotFound, "powershell.exe") => {
                Error::MissingTool("powershell.exe")
            }
            _ => io_error(&program, error),
        })?;
    loop {
        if control.state() == crate::operation::OperationState::Cancelled {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::OperationCancelled);
        }
        if child
            .try_wait()
            .map_err(|error| io_error(&program, error))?
            .is_some()
        {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| io_error(&program, error))?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            program,
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(output.stdout)
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WindowsDisk {
    number: u32,
    friendly_name: Option<String>,
    serial_number: Option<String>,
    unique_id: Option<String>,
    bus_type: String,
    size: u64,
    is_read_only: bool,
    is_boot: bool,
    is_system: bool,
}

fn discover_devices() -> Result<Vec<Device>> {
    const SCRIPT: &str = "Get-Disk | Select-Object Number,FriendlyName,SerialNumber,UniqueId,@{Name='BusType';Expression={$_.BusType.ToString()}},Size,IsReadOnly,IsBoot,IsSystem | ConvertTo-Json -Compress";
    let output = powershell(SCRIPT)?;
    parse_disks(&output)
}

fn parse_disks(output: &str) -> Result<Vec<Device>> {
    if output.trim().is_empty() || output.trim() == "null" {
        return Ok(Vec::new());
    }
    let disks: OneOrMany<WindowsDisk> =
        serde_json::from_str(output).map_err(|error| Error::InvalidToolOutput {
            program: "PowerShell Get-Disk".into(),
            message: error.to_string(),
        })?;
    Ok(disks
        .into_vec()
        .into_iter()
        .filter(|disk| disk.bus_type.eq_ignore_ascii_case("USB"))
        .filter_map(|disk| {
            let serial = clean(disk.serial_number);
            let unique_id = clean(disk.unique_id);
            let id = if let Some(value) = serial.as_deref() {
                DeviceId::new(format!("serial:{value}"))
            } else {
                let value = unique_id.as_deref()?;
                DeviceId::new(format!("windows-unique:{value}"))
            };
            Some(Device {
                id,
                path: PathBuf::from(format!(r"\\.\PhysicalDrive{}", disk.number)),
                vendor: None,
                model: clean(disk.friendly_name),
                serial,
                transport: Some("USB".into()),
                capacity: disk.size,
                removable: true,
                read_only: disk.is_read_only,
                system_disk: disk.is_boot || disk.is_system,
                mounts: Vec::new(),
            })
        })
        .collect())
}

fn refresh_target(plan: &WritePlan) -> Result<Device> {
    let target = discover_devices()?
        .into_iter()
        .find(|device| device.id == plan.target.id)
        .ok_or_else(|| Error::DeviceNotFound(plan.target.id.to_string()))?;
    if target.path != plan.target.path || target.capacity != plan.target.capacity {
        return Err(Error::StalePlan(
            "the selected Windows disk identity, path, or capacity changed".into(),
        ));
    }
    if !target.is_eligible_target() {
        return Err(Error::StalePlan(
            "the refreshed Windows disk no longer passes removable-drive safety checks".into(),
        ));
    }
    Ok(target)
}

fn physical_drive_number(path: &Path) -> Result<u32> {
    path.to_string_lossy()
        .strip_prefix(r"\\.\PhysicalDrive")
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            Error::UnsafeTarget(format!(
                "unexpected physical-drive path: {}",
                path.display()
            ))
        })
}

fn detach_drive_letters(number: u32) -> Result<()> {
    let script = format!(
        "Get-Partition -DiskNumber {number} -ErrorAction SilentlyContinue | Where-Object {{$_.DriveLetter}} | ForEach-Object {{ mountvol (\"$($_.DriveLetter):\") /p | Out-Null }}"
    );
    powershell(&script).map(|_| ())
}

fn is_administrator() -> Result<bool> {
    const SCRIPT: &str = "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)";
    Ok(powershell(SCRIPT)?.trim().eq_ignore_ascii_case("true"))
}

fn powershell(script: &str) -> Result<String> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .output()
        .map_err(|error| io_error("powershell.exe", error))?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            program: "powershell.exe".into(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_windows_tree(root: &Path) {
        for directory in ["efi/boot", "sources"] {
            fs::create_dir_all(root.join(directory)).expect("directory");
        }
        for file in [
            "efi/boot/bootx64.efi",
            "bootmgr",
            "sources/boot.wim",
            "sources/install.wim",
        ] {
            fs::write(root.join(file), b"fixture").expect("fixture");
        }
    }

    #[test]
    fn accepts_only_canonical_physical_drive_paths() {
        assert_eq!(
            physical_drive_number(Path::new(r"\\.\PhysicalDrive12")).expect("number"),
            12
        );
        assert!(physical_drive_number(Path::new(r"C:\")).is_err());
        assert!(physical_drive_number(Path::new(r"\\.\PhysicalDrive1; Clear-Disk")).is_err());
    }

    #[test]
    fn drive_letters_are_strict_and_normalized() {
        assert_eq!(drive_root("e").expect("drive"), PathBuf::from(r"E:\"));
        for value in ["", "EE", "1", "E; Clear-Disk"] {
            assert!(drive_root(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn windows_preflight_refuses_to_replace_an_answer_file() {
        let fixture = tempfile::tempdir().expect("fixture");
        synthetic_windows_tree(fixture.path());
        fs::write(fixture.path().join("AutoUnattend.XML"), b"existing").expect("answer");
        let options = WindowsExperienceOptions {
            bypass_hardware_requirements: true,
            ..WindowsExperienceOptions::default()
        };
        let result = preflight_windows_source(fixture.path(), WindowsPayload::Wim, &options);
        assert!(matches!(result, Err(Error::UnsupportedImage(_))));
    }

    #[test]
    fn windows_tree_verification_requires_boot_and_payload_files() {
        let fixture = tempfile::tempdir().expect("fixture");
        synthetic_windows_tree(fixture.path());
        verify_windows_tree(fixture.path()).expect("valid fixture");
        fs::remove_file(fixture.path().join("sources/install.wim")).expect("remove payload");
        assert!(verify_windows_tree(fixture.path()).is_err());
    }

    #[test]
    fn power_shell_inventory_keeps_only_stably_identified_usb_disks() {
        let devices = parse_disks(
            r#"[
                {"Number":2,"FriendlyName":"SanDisk","SerialNumber":" USB123 ","UniqueId":null,"BusType":"USB","Size":128000000000,"IsReadOnly":false,"IsBoot":false,"IsSystem":false},
                {"Number":3,"FriendlyName":"NVMe","SerialNumber":"NVME1","UniqueId":"NVME-ID","BusType":"NVMe","Size":1000,"IsReadOnly":false,"IsBoot":true,"IsSystem":true},
                {"Number":4,"FriendlyName":"Anonymous USB","SerialNumber":null,"UniqueId":null,"BusType":"USB","Size":1000,"IsReadOnly":false,"IsBoot":false,"IsSystem":false}
            ]"#,
        )
        .expect("inventory");

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].path, Path::new(r"\\.\PhysicalDrive2"));
        assert_eq!(devices[0].id.as_str(), "serial:USB123");
        assert!(devices[0].is_eligible_target());
    }
}
