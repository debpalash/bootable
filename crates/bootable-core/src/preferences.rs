use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::checksum::ChecksumAlgorithm;
use crate::error::{Error, Result, io_error};
use crate::model::ImageReport;

const PREFERENCES_VERSION: u32 = 1;
const MAX_RECENT_IMAGES: usize = 4;

/// An image the user inspected before. Only the path is trusted; the image is
/// inspected again when it is reused, so nothing here can bypass the write gates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentImage {
    pub path: PathBuf,
    pub size: u64,
}

impl RecentImage {
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Convenience settings shared by the desktop and terminal interfaces.
///
/// Destructive choices (target drive, bad-block testing, Windows setup answers)
/// are deliberately never remembered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    version: u32,
    pub image_directory: Option<PathBuf>,
    pub checksum_algorithm: ChecksumAlgorithm,
    recent_images: Vec<RecentImage>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: PREFERENCES_VERSION,
            image_directory: None,
            checksum_algorithm: ChecksumAlgorithm::Sha256,
            recent_images: Vec::new(),
        }
    }
}

impl Preferences {
    /// Loads the preferences, falling back to defaults when the file is absent,
    /// unreadable, corrupt, or written by an incompatible version.
    pub fn load() -> Self {
        preferences_path()
            .map(|path| Self::load_from(&path))
            .unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|preferences| preferences.version == PREFERENCES_VERSION)
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = preferences_path().ok_or_else(|| {
            Error::InvalidCatalog("no per-user configuration directory is available".into())
        })?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(directory).map_err(|error| io_error(directory, error))?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| Error::InvalidCatalog(format!("preferences: {error}")))?;
        let mut file = tempfile::NamedTempFile::new_in(directory)
            .map_err(|error| io_error(directory, error))?;
        std::io::Write::write_all(&mut file, &bytes).map_err(|error| io_error(path, error))?;
        file.persist(path)
            .map_err(|error| io_error(path, error.error))?;
        Ok(())
    }

    /// Records an inspected image: newest first, no duplicates, bounded length.
    /// Also remembers its folder as the next browse location.
    pub fn remember_image(&mut self, image: &ImageReport) {
        self.recent_images
            .retain(|recent| recent.path != image.path);
        self.recent_images.insert(
            0,
            RecentImage {
                path: image.path.clone(),
                size: image.size,
            },
        );
        self.recent_images.truncate(MAX_RECENT_IMAGES);
        if let Some(parent) = image.path.parent().filter(|parent| parent.is_dir()) {
            self.image_directory = Some(parent.to_path_buf());
        }
    }

    pub fn forget_image(&mut self, path: &Path) {
        self.recent_images.retain(|recent| recent.path != path);
    }

    /// Recent images whose files still exist, newest first.
    pub fn recent_images(&self) -> Vec<RecentImage> {
        self.recent_images
            .iter()
            .filter(|recent| recent.path.is_file())
            .cloned()
            .collect()
    }

    /// The remembered folder if it still exists.
    pub fn image_directory(&self) -> Option<PathBuf> {
        self.image_directory
            .clone()
            .filter(|directory| directory.is_dir())
    }
}

fn preferences_path() -> Option<PathBuf> {
    configuration_root().map(|root| root.join("bootable").join("preferences.json"))
}

fn configuration_root() -> Option<PathBuf> {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    if cfg!(target_os = "windows") {
        absolute("APPDATA")
    } else if cfg!(target_os = "macos") {
        absolute("HOME").map(|home| home.join("Library").join("Application Support"))
    } else {
        absolute("XDG_CONFIG_HOME").or_else(|| absolute("HOME").map(|home| home.join(".config")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ImageKind;

    fn report(path: PathBuf) -> ImageReport {
        ImageReport {
            path,
            size: 42,
            kind: ImageKind::HybridIso,
            volume_label: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn recents_are_newest_first_unique_and_bounded() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut preferences = Preferences::default();
        let paths = (0..8)
            .map(|index| {
                let path = directory.path().join(format!("image-{index}.iso"));
                fs::write(&path, b"x").expect("write");
                path
            })
            .collect::<Vec<_>>();
        for path in &paths {
            preferences.remember_image(&report(path.clone()));
        }
        preferences.remember_image(&report(paths[3].clone()));
        let recents = preferences.recent_images();
        assert_eq!(recents.len(), MAX_RECENT_IMAGES);
        assert_eq!(recents[0].path, paths[3]);
        assert_eq!(
            recents
                .iter()
                .filter(|recent| recent.path == paths[3])
                .count(),
            1
        );
        assert_eq!(
            preferences.image_directory(),
            Some(directory.path().to_path_buf())
        );
    }

    #[test]
    fn missing_files_are_hidden_and_forgotten_files_removed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let kept = directory.path().join("kept.iso");
        let gone = directory.path().join("gone.iso");
        fs::write(&kept, b"x").expect("write");
        fs::write(&gone, b"x").expect("write");
        let mut preferences = Preferences::default();
        preferences.remember_image(&report(kept.clone()));
        preferences.remember_image(&report(gone.clone()));
        fs::remove_file(&gone).expect("remove");
        assert_eq!(preferences.recent_images().len(), 1);
        preferences.forget_image(&kept);
        assert!(preferences.recent_images().is_empty());
    }

    #[test]
    fn preferences_round_trip_and_ignore_corruption() {
        let directory = tempfile::tempdir().expect("tempdir");
        let file = directory.path().join("nested").join("preferences.json");
        let image = directory.path().join("a.iso");
        fs::write(&image, b"x").expect("write");
        let mut preferences = Preferences {
            checksum_algorithm: ChecksumAlgorithm::Sha512,
            ..Preferences::default()
        };
        preferences.remember_image(&report(image.clone()));
        preferences.save_to(&file).expect("save");
        assert_eq!(Preferences::load_from(&file), preferences);

        fs::write(&file, b"{ not json").expect("corrupt");
        assert_eq!(Preferences::load_from(&file), Preferences::default());
        fs::write(
            &file,
            br#"{"version":99,"image_directory":null,"checksum_algorithm":"Md5","recent_images":[]}"#,
        )
        .expect("future");
        assert_eq!(Preferences::load_from(&file), Preferences::default());
    }
}
