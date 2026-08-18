use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use reqwest::blocking::{Client, Response};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use xz2::read::XzDecoder;

use crate::download;
use crate::error::{Error, Result, io_error};
use crate::operation::ensure_workspace;
use crate::{OperationControl, Progress, ProgressPhase};

const PI_CATALOG_URL: &str = "https://downloads.raspberrypi.com/os_list_imagingutility_v4.json";
const USER_AGENT: &str = "Bootable/0.1 (Raspberry Pi image catalog client)";
const MAX_CATALOG_BYTES: u64 = 16 * 1024 * 1024;
const BUFFER_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiDevice {
    pub name: String,
    pub tags: Vec<String>,
    pub icon_url: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiImage {
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub icon_url: Option<String>,
    pub download_url: String,
    pub archive_name: String,
    pub suggested_filename: String,
    pub download_size: Option<u64>,
    pub extracted_size: Option<u64>,
    pub release_date: Option<String>,
    pub download_sha256: Option<String>,
    pub extracted_sha256: Option<String>,
    pub devices: Vec<String>,
    pub capabilities: Vec<String>,
    pub init_format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PiCatalog {
    pub source_url: String,
    pub devices: Vec<PiDevice>,
    pub images: Vec<PiImage>,
}

#[derive(Debug, Deserialize)]
struct RawCatalog {
    imager: RawImager,
    os_list: Vec<RawImage>,
}

#[derive(Debug, Deserialize)]
struct RawImager {
    #[serde(default)]
    devices: Vec<RawDevice>,
}

#[derive(Debug, Deserialize)]
struct RawDevice {
    name: String,
    #[serde(default)]
    tags: Vec<String>,
    icon: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawImage {
    name: String,
    description: Option<String>,
    icon: Option<String>,
    url: Option<String>,
    image_download_size: Option<u64>,
    extract_size: Option<u64>,
    release_date: Option<String>,
    image_download_sha256: Option<String>,
    extract_sha256: Option<String>,
    init_format: Option<String>,
    #[serde(default)]
    devices: Vec<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    subitems: Vec<RawImage>,
}

pub(crate) fn catalog() -> Result<PiCatalog> {
    parse_catalog(&fetch_catalog()?)
}

pub(crate) fn download_image(
    image: &PiImage,
    destination: &Path,
    control: &OperationControl,
    mut progress: impl FnMut(Progress),
) -> Result<()> {
    control.checkpoint()?;
    if destination.exists() {
        return Err(Error::InvalidDownload(format!(
            "{} already exists",
            destination.display()
        )));
    }
    let parent = destination.parent().ok_or_else(|| {
        Error::InvalidDownload("download destination has no parent directory".into())
    })?;
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    ensure_workspace(
        parent,
        match (image.download_size, image.extracted_size) {
            (Some(download), Some(extracted)) => Some(download.saturating_add(extracted)),
            (_, extracted) => extracted,
        },
    )?;
    let source = secure_url(&image.download_url)?;
    progress(Progress {
        phase: ProgressPhase::Preparing,
        completed: 0,
        total: image.download_size,
        message: format!("Stage 1/6 · Connecting securely for {}", image.name),
    });
    let archive = download::stage(
        &download_client()?,
        &source,
        destination,
        image.download_size,
        control,
        |transfer| {
            progress(Progress {
                phase: ProgressPhase::Downloading,
                completed: transfer.completed,
                total: transfer.total,
                message: download::transfer_message("Stage 2/6 · Downloading", transfer),
            });
        },
    )?;
    let completed = archive.size();
    progress(Progress {
        phase: ProgressPhase::Verifying,
        completed,
        total: Some(completed),
        message: if image.download_sha256.is_some() {
            "Stage 3/6 · Verifying compressed download SHA-256".into()
        } else {
            "Stage 3/6 · Compressed checksum unavailable · transfer length verified".into()
        },
    });
    if let Err(error) = verify_sha256(
        archive.path(),
        image.download_sha256.as_deref(),
        "download",
        control,
    ) {
        archive.discard()?;
        return Err(error);
    }

    progress(Progress {
        phase: ProgressPhase::Preparing,
        completed: 0,
        total: image.extracted_size,
        message: format!("Stage 4/6 · Extracting {}", image.archive_name),
    });
    let mut extracted = tempfile::Builder::new()
        .prefix(".bootable-pi-image-")
        .suffix(".part")
        .tempfile_in(parent)
        .map_err(|error| io_error(parent, error))?;
    if let Err(error) = extract_archive(
        archive.path(),
        &image.archive_name,
        extracted.as_file_mut(),
        control,
    ) {
        archive.discard()?;
        return Err(error);
    }
    extracted
        .as_file_mut()
        .sync_all()
        .map_err(|error| io_error(destination, error))?;
    let extracted_size = extracted
        .as_file()
        .metadata()
        .map_err(|error| io_error(destination, error))?
        .len();
    progress(Progress {
        phase: ProgressPhase::Verifying,
        completed: extracted_size,
        total: image.extracted_size.or(Some(extracted_size)),
        message: if image.extracted_sha256.is_some() {
            "Stage 5/6 · Verifying extracted image SHA-256".into()
        } else {
            "Stage 5/6 · Extracted checksum unavailable · expanded size verified".into()
        },
    });
    if image
        .extracted_size
        .is_some_and(|expected| expected != extracted_size)
    {
        archive.discard()?;
        return Err(Error::InvalidDownload(format!(
            "extracted image is {extracted_size} bytes; expected {}",
            image.extracted_size.unwrap_or_default()
        )));
    }
    if let Err(error) = verify_sha256(
        extracted.path(),
        image.extracted_sha256.as_deref(),
        "extracted image",
        control,
    ) {
        archive.discard()?;
        return Err(error);
    }
    if let Err(error) = control.checkpoint() {
        archive.discard()?;
        return Err(error);
    }
    extracted
        .persist_noclobber(destination)
        .map_err(|error| io_error(destination, error.error))?;
    archive.discard()?;
    progress(Progress {
        phase: ProgressPhase::Verifying,
        completed: extracted_size,
        total: Some(extracted_size),
        message: format!(
            "Stage 5/6 · Image verified and finalized at {}",
            destination.display()
        ),
    });
    Ok(())
}

fn parse_catalog(json: &str) -> Result<PiCatalog> {
    let raw: RawCatalog = serde_json::from_str(json)
        .map_err(|error| Error::InvalidCatalog(format!("invalid Raspberry Pi catalog: {error}")))?;
    let devices = raw
        .imager
        .devices
        .into_iter()
        .map(|device| PiDevice {
            name: device.name,
            tags: device.tags,
            icon_url: device.icon.as_deref().and_then(normalize_optional_url),
            description: device.description,
        })
        .collect();
    let mut images = Vec::new();
    for image in raw.os_list {
        flatten_image(image, None, None, &[], &mut images);
    }
    if images.is_empty() {
        return Err(Error::InvalidCatalog(
            "Raspberry Pi catalog contains no supported images".into(),
        ));
    }
    Ok(PiCatalog {
        source_url: PI_CATALOG_URL.into(),
        devices,
        images,
    })
}

fn flatten_image(
    image: RawImage,
    parent_category: Option<String>,
    parent_icon: Option<String>,
    parent_devices: &[String],
    output: &mut Vec<PiImage>,
) {
    let icon = image
        .icon
        .as_deref()
        .and_then(normalize_optional_url)
        .or(parent_icon);
    let devices = if image.devices.is_empty() {
        parent_devices.to_vec()
    } else {
        image.devices.clone()
    };
    if let Some(url) = image.url.as_deref().and_then(normalize_optional_url)
        && let Some(archive_name) = archive_name(&url)
        && supported_archive(&archive_name)
    {
        output.push(PiImage {
            name: image.name.clone(),
            description: image.description.clone(),
            category: parent_category.clone(),
            icon_url: icon.clone(),
            suggested_filename: suggested_filename(&archive_name, &image.name),
            archive_name,
            download_url: url,
            download_size: image.image_download_size,
            extracted_size: image.extract_size,
            release_date: image.release_date.clone(),
            download_sha256: image.image_download_sha256.clone(),
            extracted_sha256: image.extract_sha256.clone(),
            devices: devices.clone(),
            capabilities: image.capabilities.clone(),
            init_format: image.init_format.clone(),
        });
    }
    let category = Some(match parent_category {
        Some(parent) => format!("{parent} · {}", image.name),
        None => image.name.clone(),
    });
    for child in image.subitems {
        flatten_image(child, category.clone(), icon.clone(), &devices, output);
    }
}

fn normalize_optional_url(value: &str) -> Option<String> {
    let mut url = Url::parse(value).ok()?;
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    (url.scheme() == "https" && url.host_str().is_some()).then(|| url.to_string())
}

fn archive_name(value: &str) -> Option<String> {
    Url::parse(value)
        .ok()?
        .path_segments()?
        .next_back()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn supported_archive(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".img", ".iso", ".xz", ".gz", ".bz2", ".zst", ".zip"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn suggested_filename(archive: &str, display_name: &str) -> String {
    let mut name = archive.to_owned();
    for suffix in [".xz", ".gz", ".bz2", ".zst", ".zip"] {
        if name.to_ascii_lowercase().ends_with(suffix) {
            name.truncate(name.len() - suffix.len());
            break;
        }
    }
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".img") || lower.ends_with(".iso") {
        return name;
    }
    let safe = display_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("{}.img", safe.trim_matches('-'))
}

fn extract_archive(
    path: &Path,
    archive_name: &str,
    output: &mut File,
    control: &OperationControl,
) -> Result<()> {
    let lower = archive_name.to_ascii_lowercase();
    let input = File::open(path).map_err(|error| io_error(path, error))?;
    if lower.ends_with(".xz") {
        copy_decoder(XzDecoder::new(input), output, path, control)
    } else if lower.ends_with(".gz") {
        copy_decoder(GzDecoder::new(input), output, path, control)
    } else if lower.ends_with(".bz2") {
        copy_decoder(BzDecoder::new(input), output, path, control)
    } else if lower.ends_with(".zst") {
        let decoder = zstd::stream::read::Decoder::new(input)
            .map_err(|error| Error::InvalidDownload(format!("open zstd image: {error}")))?;
        copy_decoder(decoder, output, path, control)
    } else if lower.ends_with(".zip") {
        extract_zip(input, output, path, control)
    } else {
        copy_decoder(input, output, path, control)
    }
}

fn copy_decoder(
    mut input: impl Read,
    output: &mut File,
    path: &Path,
    control: &OperationControl,
) -> Result<()> {
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        control.checkpoint()?;
        let count = input
            .read(&mut buffer)
            .map_err(|error| io_error(path, error))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| io_error(path, error))?;
    }
    Ok(())
}

fn extract_zip(
    input: File,
    output: &mut File,
    path: &Path,
    control: &OperationControl,
) -> Result<()> {
    let mut archive = zip::ZipArchive::new(input)
        .map_err(|error| Error::InvalidDownload(format!("open ZIP image: {error}")))?;
    let index = (0..archive.len())
        .find(|index| {
            archive.by_index(*index).ok().is_some_and(|entry| {
                let name = entry.name().to_ascii_lowercase();
                name.ends_with(".img") || name.ends_with(".iso")
            })
        })
        .ok_or_else(|| Error::InvalidDownload("ZIP archive contains no IMG or ISO file".into()))?;
    let mut entry = archive
        .by_index(index)
        .map_err(|error| Error::InvalidDownload(format!("read ZIP image: {error}")))?;
    copy_decoder(&mut entry, output, path, control)
}

fn verify_sha256(
    path: &Path,
    expected: Option<&str>,
    label: &str,
    control: &OperationControl,
) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        control.checkpoint()?;
        let count = file
            .read(&mut buffer)
            .map_err(|error| io_error(path, error))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(Error::InvalidDownload(format!(
            "SHA-256 mismatch for {label}"
        )));
    }
    Ok(())
}

fn fetch_catalog() -> Result<String> {
    let response = send(&metadata_client()?, PI_CATALOG_URL)?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_CATALOG_BYTES)
    {
        return Err(Error::InvalidCatalog(
            "Raspberry Pi catalog is too large".into(),
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_CATALOG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| network_error(PI_CATALOG_URL, error))?;
    if bytes.len() as u64 > MAX_CATALOG_BYTES {
        return Err(Error::InvalidCatalog(
            "Raspberry Pi catalog is too large".into(),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|error| Error::InvalidCatalog(format!("Pi catalog is not UTF-8: {error}")))
}

fn client_builder() -> reqwest::blocking::ClientBuilder {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .redirect(Policy::limited(10))
}

fn metadata_client() -> Result<Client> {
    client_builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| network_error("client setup", error))
}

fn download_client() -> Result<Client> {
    client_builder()
        .build()
        .map_err(|error| network_error("download client setup", error))
}

fn send(client: &Client, url: &str) -> Result<Response> {
    let response = client
        .get(url)
        .send()
        .and_then(Response::error_for_status)
        .map_err(|error| network_error(url, error))?;
    if response.url().scheme() != "https" {
        return Err(Error::InvalidDownload(
            "Raspberry Pi catalog request redirected to an insecure URL".into(),
        ));
    }
    Ok(response)
}

fn secure_url(value: &str) -> Result<Url> {
    let url = Url::parse(value)
        .map_err(|error| Error::InvalidDownload(format!("invalid URL: {error}")))?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err(Error::InvalidDownload(
            "Raspberry Pi downloads require an absolute HTTPS URL".into(),
        ));
    }
    Ok(url)
}

fn network_error(url: &str, error: impl std::fmt::Display) -> Error {
    Error::Network {
        url: url.into(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_nested_images_and_upgrades_https() {
        let json = r#"{
          "imager":{"devices":[{"name":"Pi 5","tags":["pi5-64bit"],"icon":"https://example.com/pi.png"}]},
          "os_list":[{"name":"Other","description":"category","subitems":[{
            "name":"Test OS","description":"fast","icon":"https://example.com/os.png",
            "url":"http://example.com/test.img.xz","extract_size":42,"image_download_size":21,
            "extract_sha256":"abcd","devices":["pi5-64bit"]
          }]}]
        }"#;
        let catalog = parse_catalog(json).expect("catalog");
        assert_eq!(catalog.devices.len(), 1);
        assert_eq!(catalog.images.len(), 1);
        assert_eq!(catalog.images[0].category.as_deref(), Some("Other"));
        assert_eq!(catalog.images[0].suggested_filename, "test.img");
        assert!(catalog.images[0].download_url.starts_with("https://"));
    }

    #[test]
    fn rejects_catalog_without_supported_images() {
        let json = r#"{"imager":{"devices":[]},"os_list":[{"name":"Docs","url":"https://example.com/readme.txt"}]}"#;
        assert!(parse_catalog(json).is_err());
    }
}
