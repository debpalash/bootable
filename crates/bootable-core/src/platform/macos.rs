use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
            WriteStrategy::WindowsFat32 { .. } => Err(Error::PlatformUnavailable(
                "native macOS Windows installer extraction/formatting is not implemented yet"
                    .into(),
            )),
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
    part_of_whole: Option<String>,
}

fn discover_devices() -> Result<Vec<Device>> {
    let root_disk = root_disk()?;
    let output = Command::new("ioreg")
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
    let output = Command::new("diskutil")
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
    let output = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|error| io_error("id", error))?;
    Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "0")
}

fn run_diskutil<const N: usize>(arguments: [&str; N]) -> Result<()> {
    let output = Command::new("diskutil")
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
    let mut child = Command::new("plutil")
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
