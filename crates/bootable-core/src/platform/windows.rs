use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::error::{Error, Result, io_error};
use crate::model::{Device, DeviceId, Progress, WritePlan, WriteStrategy};
use crate::operation::OperationControl;

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
        let target = refresh_target(plan)?;
        let number = physical_drive_number(&target.path)?;
        detach_drive_letters(number)?;
        control.checkpoint()?;
        match plan.strategy {
            WriteStrategy::RawVerified => {
                super::raw::write(&plan.image, &target.path, control, progress)
            }
            WriteStrategy::WindowsFat32 { .. } => Err(Error::PlatformUnavailable(
                "native Windows installer extraction/formatting is not implemented yet".into(),
            )),
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
