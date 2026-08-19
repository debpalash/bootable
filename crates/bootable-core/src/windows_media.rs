use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result, io_error};
use crate::model::{WindowsExperienceOptions, WindowsPayload};

pub(crate) const FAT32_MAX_FILE_SIZE: u64 = u32::MAX as u64;

pub(crate) fn apply_setup_options(root: &Path, options: &WindowsExperienceOptions) -> Result<()> {
    if !options.requires_answer_file() {
        return Ok(());
    }
    let path = root.join("autounattend.xml");
    if path.exists() {
        return Err(Error::UnsupportedImage(
            "the source already contains autounattend.xml; refusing to overwrite it".into(),
        ));
    }
    fs::write(&path, crate::windows::answer_file(options)?).map_err(|error| io_error(path, error))
}

pub(crate) fn find_install_payload(root: &Path, payload: WindowsPayload) -> Result<PathBuf> {
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

pub(crate) fn find_case_insensitive_child(parent: &Path, name: &str) -> Result<PathBuf> {
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

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(crate) fn find_optional_case_insensitive_child(parent: &Path, name: &str) -> Option<PathBuf> {
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

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(crate) fn reject_oversized_files_except(path: &Path, payload: &Path) -> Result<()> {
    for entry in fs::read_dir(path).map_err(|error| io_error(path, error))? {
        let entry = entry.map_err(|error| io_error(path, error))?;
        let child = entry.path();
        if child == payload {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| io_error(&child, error))?;
        if file_type.is_dir() {
            reject_oversized_files_except(&child, payload)?;
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

pub(crate) fn verify_written_tree(root: &Path) -> Result<()> {
    let efi = find_case_insensitive_child(root, "efi")?;
    let efi_boot = find_case_insensitive_child(&efi, "boot")?;
    if !fs::read_dir(&efi_boot)
        .map_err(|error| io_error(&efi_boot, error))?
        .filter_map(std::result::Result::ok)
        .any(|entry| {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            name.starts_with("boot") && name.ends_with(".efi")
        })
    {
        return Err(Error::UnsupportedImage(
            "written media is missing efi/boot/boot*.efi".into(),
        ));
    }
    find_case_insensitive_child(root, "bootmgr")?;
    let sources = find_case_insensitive_child(root, "sources")?;
    find_case_insensitive_child(&sources, "boot.wim")?;
    if !fs::read_dir(&sources)
        .map_err(|error| io_error(&sources, error))?
        .filter_map(std::result::Result::ok)
        .any(|entry| {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            name == "install.wim"
                || name == "install.esd"
                || (name.starts_with("install") && name.ends_with(".swm"))
        })
    {
        return Err(Error::UnsupportedImage(
            "written media is missing its Windows install payload".into(),
        ));
    }
    verify_fat_file_sizes(root)
}

pub(crate) fn verify_fat_file_sizes(path: &Path) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_a_complete_windows_tree_through_one_interface() {
        let root = tempfile::tempdir().expect("fixture");
        fs::create_dir_all(root.path().join("EFI/Boot")).expect("EFI tree");
        fs::create_dir_all(root.path().join("Sources")).expect("sources tree");
        fs::write(root.path().join("EFI/Boot/BOOTX64.EFI"), b"efi").expect("loader");
        fs::write(root.path().join("bootmgr"), b"boot").expect("boot manager");
        fs::write(root.path().join("Sources/boot.wim"), b"boot wim").expect("boot WIM");
        fs::write(root.path().join("Sources/install.esd"), b"payload").expect("payload");

        verify_written_tree(root.path()).expect("valid Windows media");
        fs::remove_file(root.path().join("bootmgr")).expect("remove boot manager");
        assert!(verify_written_tree(root.path()).is_err());
    }

    #[test]
    fn setup_options_refuse_to_replace_an_existing_answer_file() {
        let root = tempfile::tempdir().expect("fixture");
        let options = WindowsExperienceOptions {
            bypass_hardware_requirements: true,
            ..WindowsExperienceOptions::default()
        };
        apply_setup_options(root.path(), &options).expect("first answer file");
        assert!(apply_setup_options(root.path(), &options).is_err());
    }
}
