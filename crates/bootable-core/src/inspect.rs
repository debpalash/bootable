use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result, io_error};
use crate::model::{CompressedImageKind, ImageCompression, ImageKind, ImageReport, WindowsPayload};

const ISO_PRIMARY_VOLUME_OFFSET: u64 = 16 * 2048;
const ISO_HEADER_LEN: usize = 6;

pub(crate) fn inspect(path: &Path) -> Result<ImageReport> {
    let metadata = path.metadata().map_err(|error| io_error(path, error))?;
    if !metadata.is_file() {
        return Err(Error::UnsupportedImage(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() == 0 {
        return Err(Error::UnsupportedImage("the image is empty".into()));
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    if let Some(compression) = compression_for_extension(extension.as_deref()) {
        return inspect_compressed(path, metadata.len(), compression);
    }
    if matches!(extension.as_deref(), Some("img" | "raw")) {
        return Ok(ImageReport {
            path: path.to_path_buf(),
            size: metadata.len(),
            kind: ImageKind::RawDiskImage,
            volume_label: None,
            warnings: Vec::new(),
        });
    }

    ensure_iso_signature(path)?;
    let entries = image_entries(path)?;
    let lower_entries = entries
        .iter()
        .map(|entry| entry.path.to_ascii_lowercase())
        .collect::<Vec<_>>();

    let payload = if let Some(size) = entry_size(&entries, "/sources/install.wim") {
        Some((WindowsPayload::Wim, size))
    } else if let Some(size) = entry_size(&entries, "/sources/install.esd") {
        Some((WindowsPayload::Esd, size))
    } else if lower_entries
        .iter()
        .any(|entry| entry.contains("/sources/install") && entry.ends_with(".swm"))
    {
        let total_size = entries
            .iter()
            .filter(|entry| {
                let path = entry.path.to_ascii_lowercase();
                path.contains("/sources/install") && path.ends_with(".swm")
            })
            .filter_map(|entry| entry.size)
            .sum::<u64>();
        Some((
            WindowsPayload::SplitWim,
            (total_size > 0).then_some(total_size),
        ))
    } else {
        None
    };

    let has_windows_boot = has_suffix(&lower_entries, "/bootmgr")
        && lower_entries
            .iter()
            .any(|entry| entry.starts_with("/efi/boot/boot") && entry.ends_with(".efi"));
    let kind = if has_windows_boot {
        let (payload, payload_size) = payload.ok_or_else(|| {
            Error::UnsupportedImage(
                "Windows media has no install.wim, install.esd, or install*.swm payload".into(),
            )
        })?;
        ImageKind::WindowsInstaller {
            payload,
            payload_size,
        }
    } else if has_hybrid_partition_table(path)? {
        ImageKind::HybridIso
    } else {
        ImageKind::OpticalIso
    };

    let warnings = match kind {
        ImageKind::OpticalIso => {
            vec!["This ISO has no hybrid USB partition table; raw writing may not boot.".into()]
        }
        _ => Vec::new(),
    };

    Ok(ImageReport {
        path: path.to_path_buf(),
        size: metadata.len(),
        kind,
        volume_label: None,
        warnings,
    })
}

fn compression_for_extension(extension: Option<&str>) -> Option<ImageCompression> {
    match extension {
        Some("xz") => Some(ImageCompression::Xz),
        Some("gz" | "gzip") => Some(ImageCompression::Gzip),
        Some("zst" | "zstd") => Some(ImageCompression::Zstandard),
        Some("bz2" | "bzip2") => Some(ImageCompression::Bzip2),
        _ => None,
    }
}

pub(crate) fn compressed_reader(
    path: &Path,
    compression: ImageCompression,
) -> Result<Box<dyn Read>> {
    let file = File::open(path).map_err(|error| io_error(path, error))?;
    let reader: Box<dyn Read> = match compression {
        ImageCompression::Xz => Box::new(xz2::read::XzDecoder::new(BufReader::new(file))),
        ImageCompression::Gzip => Box::new(flate2::read::GzDecoder::new(BufReader::new(file))),
        ImageCompression::Zstandard => {
            Box::new(zstd::stream::read::Decoder::new(file).map_err(|error| io_error(path, error))?)
        }
        ImageCompression::Bzip2 => Box::new(bzip2::read::BzDecoder::new(BufReader::new(file))),
    };
    Ok(reader)
}

fn inspect_compressed(
    path: &Path,
    compressed_size: u64,
    compression: ImageCompression,
) -> Result<ImageReport> {
    const HEADER_CAPTURE: usize = 64 * 1024;
    let mut reader = compressed_reader(path, compression)?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut header = Vec::with_capacity(HEADER_CAPTURE);
    let mut expanded_size = 0_u64;
    loop {
        let count = reader.read(&mut buffer).map_err(|error| {
            Error::UnsupportedImage(format!(
                "could not decompress {} as {compression}: {error}",
                path.display()
            ))
        })?;
        if count == 0 {
            break;
        }
        expanded_size = expanded_size
            .checked_add(count as u64)
            .ok_or_else(|| Error::UnsupportedImage("expanded image is too large".into()))?;
        if header.len() < HEADER_CAPTURE {
            let retained = count.min(HEADER_CAPTURE - header.len());
            header.extend_from_slice(&buffer[..retained]);
        }
    }
    if expanded_size == 0 {
        return Err(Error::UnsupportedImage(
            "the compressed image expands to an empty file".into(),
        ));
    }

    let inner_extension = path
        .file_stem()
        .map(Path::new)
        .and_then(Path::extension)
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let iso_signature = header
        .get(ISO_PRIMARY_VOLUME_OFFSET as usize..ISO_PRIMARY_VOLUME_OFFSET as usize + 6)
        .is_some_and(|value| &value[1..] == b"CD001");
    let hybrid = has_hybrid_partition_header(&header);
    let inner = if iso_signature {
        if !hybrid {
            return Err(Error::UnsupportedImage(
                "compressed optical-only ISOs must be decompressed before conversion; compressed hybrid ISOs can be streamed directly"
                    .into(),
            ));
        }
        CompressedImageKind::HybridIso
    } else if matches!(inner_extension.as_deref(), Some("iso")) {
        return Err(Error::UnsupportedImage(
            "the expanded .iso payload has no ISO-9660 signature".into(),
        ));
    } else {
        CompressedImageKind::RawDiskImage
    };

    Ok(ImageReport {
        path: path.to_path_buf(),
        size: expanded_size,
        kind: ImageKind::CompressedDiskImage { compression, inner },
        volume_label: None,
        warnings: vec![format!(
            "Compressed source is {}; the target requires {} after expansion.",
            crate::model::format_bytes(compressed_size),
            crate::model::format_bytes(expanded_size)
        )],
    })
}

fn ensure_iso_signature(path: &Path) -> Result<()> {
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    file.seek(SeekFrom::Start(ISO_PRIMARY_VOLUME_OFFSET))
        .map_err(|error| io_error(path, error))?;
    let mut header = [0_u8; ISO_HEADER_LEN];
    file.read_exact(&mut header)
        .map_err(|error| io_error(path, error))?;
    if &header[1..] != b"CD001" {
        return Err(Error::UnsupportedImage(format!(
            "{} is neither an ISO-9660 image nor a .img/.raw disk image",
            path.display()
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageEntry {
    path: String,
    size: Option<u64>,
}

fn image_entries(path: &Path) -> Result<Vec<ImageEntry>> {
    if has_udf_signature(path)? {
        return seven_zip_entries(path);
    }
    match xorriso_entries(path) {
        Ok(entries) if entries.len() > 1 => Ok(entries),
        Ok(_) | Err(Error::MissingTool(_)) | Err(Error::CommandFailed { .. }) => {
            seven_zip_entries(path)
        }
        Err(error) => Err(error),
    }
}

fn has_udf_signature(path: &Path) -> Result<bool> {
    const UDF_SCAN_SIZE: usize = 256 * 1024;
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    file.seek(SeekFrom::Start(ISO_PRIMARY_VOLUME_OFFSET))
        .map_err(|error| io_error(path, error))?;
    let mut buffer = vec![0_u8; UDF_SCAN_SIZE];
    let count = file
        .read(&mut buffer)
        .map_err(|error| io_error(path, error))?;
    Ok(buffer[..count]
        .windows(5)
        .any(|window| window == b"NSR02" || window == b"NSR03"))
}

fn xorriso_entries(path: &Path) -> Result<Vec<ImageEntry>> {
    let output = Command::new("xorriso")
        .args(["-indev"])
        .arg(path)
        .args(["-find", "/", "-type", "f", "-exec", "echo", "--"])
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::MissingTool("xorriso")
            } else {
                io_error("xorriso", error)
            }
        })?;
    if !output.status.success() {
        return Err(Error::CommandFailed {
            program: "xorriso".into(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| ImageEntry {
            path: normalize_entry_path(line.trim_matches('\'')),
            size: None,
        })
        .filter(|entry| !entry.path.is_empty())
        .collect())
}

fn seven_zip_entries(path: &Path) -> Result<Vec<ImageEntry>> {
    for program in ["7zz", "7z"] {
        let output = match Command::new(program).args(["l", "-slt"]).arg(path).output() {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_error(program, error)),
        };
        if !output.status.success() {
            return Err(Error::CommandFailed {
                program: program.into(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        return Ok(parse_seven_zip_listing(&String::from_utf8_lossy(
            &output.stdout,
        )));
    }
    Err(Error::MissingTool("7z or 7zz"))
}

fn parse_seven_zip_listing(listing: &str) -> Vec<ImageEntry> {
    let mut current_path = None;
    let mut entries = Vec::new();
    for line in listing.lines() {
        if let Some(path) = line.strip_prefix("Path = ") {
            current_path = Some(normalize_entry_path(path));
        } else if let Some(size) = line.strip_prefix("Size = ")
            && let (Some(path), Ok(size)) = (current_path.take(), size.parse::<u64>())
        {
            entries.push(ImageEntry {
                path,
                size: Some(size),
            });
        }
    }
    entries
}

fn normalize_entry_path(path: &str) -> String {
    let path = path.trim().replace('\\', "/");
    if path.is_empty() || path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    }
}

fn entry_size(entries: &[ImageEntry], suffix: &str) -> Option<Option<u64>> {
    entries
        .iter()
        .find(|entry| entry.path.to_ascii_lowercase().ends_with(suffix))
        .map(|entry| entry.size)
}

fn has_suffix(entries: &[String], suffix: &str) -> bool {
    entries.iter().any(|entry| entry.ends_with(suffix))
}

fn has_hybrid_partition_table(path: &Path) -> Result<bool> {
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    let mut sectors = [0_u8; 1024];
    file.read_exact(&mut sectors)
        .map_err(|error| io_error(path, error))?;
    Ok(has_hybrid_partition_header(&sectors))
}

fn has_hybrid_partition_header(sectors: &[u8]) -> bool {
    if sectors.len() < 1024 {
        return false;
    }
    let mbr = sectors[510..512] == [0x55, 0xaa]
        && sectors[446..510]
            .as_chunks::<16>()
            .0
            .iter()
            .any(|entry| entry[4] != 0 && entry[12..16] != [0, 0, 0, 0]);
    let gpt = &sectors[512..520] == b"EFI PART";
    mbr || gpt
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn recognizes_raw_extensions_without_external_tools() {
        let mut image = tempfile::Builder::new()
            .suffix(".img")
            .tempfile()
            .expect("temp image");
        image.write_all(&[1, 2, 3]).expect("write image");

        let report = inspect(image.path()).expect("inspect image");

        assert_eq!(report.kind, ImageKind::RawDiskImage);
        assert_eq!(report.size, 3);
    }

    #[test]
    fn rejects_an_empty_image() {
        let image = NamedTempFile::new().expect("temp image");
        let error = inspect(image.path()).expect_err("empty image should fail");
        assert!(matches!(error, Error::UnsupportedImage(_)));
    }

    #[test]
    fn inspects_xz_images_by_their_expanded_size() {
        let image = tempfile::Builder::new()
            .suffix(".img.xz")
            .tempfile()
            .expect("compressed image");
        let mut encoder = xz2::write::XzEncoder::new(image.reopen().expect("reopen"), 1);
        encoder.write_all(&vec![0x5a; 8192]).expect("compress");
        encoder.finish().expect("finish compression");

        let report = inspect(image.path()).expect("inspect compressed image");

        assert_eq!(report.size, 8192);
        assert!(matches!(
            report.kind,
            ImageKind::CompressedDiskImage {
                compression: ImageCompression::Xz,
                inner: CompressedImageKind::RawDiskImage,
            }
        ));
        assert_eq!(report.warnings.len(), 1);
    }

    #[test]
    fn compressed_hybrid_iso_is_streamable_but_optical_iso_is_not() {
        let mut bytes = vec![0_u8; 64 * 1024];
        bytes[ISO_PRIMARY_VOLUME_OFFSET as usize..ISO_PRIMARY_VOLUME_OFFSET as usize + 6]
            .copy_from_slice(&[1, b'C', b'D', b'0', b'0', b'1']);
        let optical = tempfile::Builder::new()
            .suffix(".iso.gz")
            .tempfile()
            .expect("optical image");
        let mut encoder = flate2::write::GzEncoder::new(
            optical.reopen().expect("reopen"),
            flate2::Compression::fast(),
        );
        encoder.write_all(&bytes).expect("compress optical");
        encoder.finish().expect("finish optical");
        assert!(matches!(
            inspect(optical.path()),
            Err(Error::UnsupportedImage(_))
        ));

        bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
        bytes[446 + 4] = 0x17;
        bytes[446 + 12] = 1;
        let hybrid = tempfile::Builder::new()
            .suffix(".iso.gz")
            .tempfile()
            .expect("hybrid image");
        let mut encoder = flate2::write::GzEncoder::new(
            hybrid.reopen().expect("reopen"),
            flate2::Compression::fast(),
        );
        encoder.write_all(&bytes).expect("compress hybrid");
        encoder.finish().expect("finish hybrid");

        let report = inspect(hybrid.path()).expect("inspect hybrid");
        assert!(matches!(
            report.kind,
            ImageKind::CompressedDiskImage {
                compression: ImageCompression::Gzip,
                inner: CompressedImageKind::HybridIso,
            }
        ));
    }

    #[test]
    fn zstandard_and_bzip2_raw_images_are_measured() {
        let directory = tempfile::tempdir().expect("directory");
        let payload = vec![0x39_u8; 12_345];
        let zstandard = directory.path().join("disk.img.zst");
        let mut zstandard_encoder =
            zstd::stream::write::Encoder::new(File::create(&zstandard).expect("zstd file"), 1)
                .expect("zstd encoder");
        zstandard_encoder.write_all(&payload).expect("zstd payload");
        zstandard_encoder.finish().expect("finish zstd");
        let bzip2 = directory.path().join("disk.raw.bz2");
        let mut bzip2_encoder = bzip2::write::BzEncoder::new(
            File::create(&bzip2).expect("bzip2 file"),
            bzip2::Compression::fast(),
        );
        bzip2_encoder.write_all(&payload).expect("bzip2 payload");
        bzip2_encoder.finish().expect("finish bzip2");

        for (path, compression) in [
            (zstandard, ImageCompression::Zstandard),
            (bzip2, ImageCompression::Bzip2),
        ] {
            let report = inspect(&path).expect("inspect compressed raw image");
            assert_eq!(report.size, payload.len() as u64);
            assert_eq!(
                report.kind,
                ImageKind::CompressedDiskImage {
                    compression,
                    inner: CompressedImageKind::RawDiskImage,
                }
            );
        }
    }

    #[test]
    fn parses_windows_udf_entries_from_seven_zip() {
        let listing = r#"
Path = windows.iso
Type = Udf

----------
Path = bootmgr
Size = 473364
Path = efi/boot/bootx64.efi
Size = 2855368
Path = sources/install.wim
Size = 6868632137
"#;

        let entries = parse_seven_zip_listing(listing);

        assert_eq!(entry_size(&entries, "/bootmgr"), Some(Some(473_364)));
        assert_eq!(
            entry_size(&entries, "/sources/install.wim"),
            Some(Some(6_868_632_137))
        );
    }
}
