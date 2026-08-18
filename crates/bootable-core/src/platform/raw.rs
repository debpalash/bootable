use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result, io_error};
use crate::model::{ImageKind, ImageReport, Progress, ProgressPhase};
use crate::operation::OperationControl;

const BUFFER_SIZE: usize = 4 * 1024 * 1024;

pub(crate) fn write(
    image: &ImageReport,
    target_path: &Path,
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let mut source: Box<dyn Read> = match image.kind {
        ImageKind::CompressedDiskImage { compression, .. } => {
            crate::inspect::compressed_reader(&image.path, compression)?
        }
        _ => Box::new(BufReader::with_capacity(
            BUFFER_SIZE,
            File::open(&image.path).map_err(|error| io_error(image.path.clone(), error))?,
        )),
    };
    let target_file = OpenOptions::new()
        .write(true)
        .open(target_path)
        .map_err(|error| io_error(target_path, error))?;
    let mut destination = BufWriter::with_capacity(BUFFER_SIZE, target_file);
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    let mut written = 0_u64;
    let mut source_hash = Sha256::new();

    loop {
        if let Err(error) = control.checkpoint() {
            destination
                .flush()
                .map_err(|flush_error| io_error(target_path, flush_error))?;
            destination
                .get_ref()
                .sync_all()
                .map_err(|sync_error| io_error(target_path, sync_error))?;
            return Err(error);
        }
        let count = source
            .read(&mut buffer)
            .map_err(|error| io_error(image.path.clone(), error))?;
        if count == 0 {
            break;
        }
        destination
            .write_all(&buffer[..count])
            .map_err(|error| io_error(target_path, error))?;
        source_hash.update(&buffer[..count]);
        written = written.saturating_add(count as u64);
        if written > image.size {
            flush_before_error(&mut destination, target_path)?;
            return Err(Error::StalePlan(
                "the source expands beyond its inspected size; refusing to continue".into(),
            ));
        }
        progress(Progress {
            phase: ProgressPhase::Writing,
            completed: written,
            total: Some(image.size),
            message: "Writing image".into(),
        });
    }
    if written != image.size {
        flush_before_error(&mut destination, target_path)?;
        return Err(Error::StalePlan(format!(
            "the source now expands to {written} bytes instead of the inspected {} bytes",
            image.size
        )));
    }
    flush_before_error(&mut destination, target_path)?;
    drop(destination);

    progress(Progress {
        phase: ProgressPhase::Syncing,
        completed: image.size,
        total: Some(image.size),
        message: "Writes flushed; starting byte verification".into(),
    });
    control.checkpoint()?;
    let expected_hash: [u8; 32] = source_hash.finalize().into();
    verify_target_hash(target_path, image.size, expected_hash, control, progress)?;
    progress(Progress {
        phase: ProgressPhase::Finished,
        completed: image.size,
        total: Some(image.size),
        message: "Image written and byte-verified".into(),
    });
    Ok(())
}

fn flush_before_error(destination: &mut BufWriter<File>, target_path: &Path) -> Result<()> {
    destination
        .flush()
        .map_err(|error| io_error(target_path, error))?;
    destination
        .get_ref()
        .sync_all()
        .map_err(|error| io_error(target_path, error))
}

pub(crate) fn verify_target_hash(
    target_path: &Path,
    size: u64,
    expected_hash: [u8; 32],
    control: &OperationControl,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    let mut target = BufReader::with_capacity(
        BUFFER_SIZE,
        File::open(target_path).map_err(|error| io_error(target_path, error))?,
    );
    let mut target_hash = Sha256::new();
    let mut target_buffer = vec![0_u8; BUFFER_SIZE];
    let mut verified = 0_u64;

    while verified < size {
        control.checkpoint()?;
        let requested = usize::try_from((size - verified).min(BUFFER_SIZE as u64))
            .map_err(|_| Error::UnsupportedImage("image size is not addressable".into()))?;
        target
            .read_exact(&mut target_buffer[..requested])
            .map_err(|error| io_error(target_path, error))?;
        target_hash.update(&target_buffer[..requested]);
        verified += requested as u64;
        progress(Progress {
            phase: ProgressPhase::Verifying,
            completed: verified,
            total: Some(size),
            message: "Comparing SHA-256 digests".into(),
        });
    }
    if expected_hash.as_slice() != target_hash.finalize().as_slice() {
        return Err(Error::StalePlan(
            "verification failed: source and target SHA-256 digests differ".into(),
        ));
    }
    Ok(())
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn backup(
    source_path: &Path,
    size: u64,
    destination: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<()> {
    if destination.exists() {
        return Err(Error::UnsafeTarget(format!(
            "{} already exists; refusing to overwrite a backup",
            destination.display()
        )));
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let mut source = BufReader::with_capacity(
        BUFFER_SIZE,
        File::open(source_path).map_err(|error| io_error(source_path, error))?,
    );
    let temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| io_error(parent, error))?;
    let mut output = BufWriter::with_capacity(
        BUFFER_SIZE,
        temporary
            .reopen()
            .map_err(|error| io_error(destination, error))?,
    );
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    let mut copied = 0_u64;
    while copied < size {
        let requested = usize::try_from((size - copied).min(BUFFER_SIZE as u64))
            .map_err(|_| Error::UnsupportedImage("device size is not addressable".into()))?;
        source
            .read_exact(&mut buffer[..requested])
            .map_err(|error| io_error(source_path, error))?;
        output
            .write_all(&buffer[..requested])
            .map_err(|error| io_error(destination, error))?;
        copied += requested as u64;
        progress(Progress {
            phase: ProgressPhase::Reading,
            completed: copied,
            total: Some(size),
            message: "Backing up removable media".into(),
        });
    }
    output
        .flush()
        .map_err(|error| io_error(destination, error))?;
    output
        .get_ref()
        .sync_all()
        .map_err(|error| io_error(destination, error))?;
    drop(output);
    temporary
        .persist_noclobber(destination)
        .map_err(|error| io_error(destination, error.error))?;
    progress(Progress {
        phase: ProgressPhase::Finished,
        completed: size,
        total: Some(size),
        message: "Drive backup completed atomically".into(),
    });
    Ok(())
}
