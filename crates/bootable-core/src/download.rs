use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use reqwest::header::{CONTENT_RANGE, ETAG, IF_RANGE, LAST_MODIFIED, RANGE};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::catalog::IsoRelease;
use crate::error::{Error, Result, io_error};
use crate::pi_catalog::PiImage;
use crate::{OperationControl, Progress};

const LEDGER_VERSION: u32 = 1;
const PARTIAL_VERSION: u32 = 1;
const BUFFER_SIZE: usize = 1024 * 1024;
const MAX_RECONNECTS: usize = 3;
const PROGRESS_SAVE_INTERVAL: Duration = Duration::from_secs(1);
const ACTIVE_LEASE_MILLIS: u64 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadKind {
    Iso,
    RaspberryPi,
}

impl std::fmt::Display for DownloadKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Iso => "ISO",
            Self::RaspberryPi => "Raspberry Pi image",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadStatus {
    Queued,
    Running,
    Paused,
    Interrupted,
    Completed,
    Failed,
    Cancelled,
}

impl DownloadStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub fn can_retry(self) -> bool {
        matches!(self, Self::Interrupted | Self::Failed | Self::Cancelled)
    }
}

impl std::fmt::Display for DownloadStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Queued => "Queued",
            Self::Running => "Downloading",
            Self::Paused => "Paused",
            Self::Interrupted => "Interrupted",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadJob {
    pub id: String,
    pub kind: DownloadKind,
    pub label: String,
    pub destination: PathBuf,
    pub status: DownloadStatus,
    pub completed: u64,
    pub total: Option<u64>,
    pub message: String,
    pub error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl DownloadJob {
    pub fn progress_ratio(&self) -> Option<f64> {
        self.total
            .filter(|total| *total > 0)
            .map(|total| (self.completed as f64 / total as f64).clamp(0., 1.))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum DownloadPayload {
    Iso(IsoRelease),
    RaspberryPi(PiImage),
}

impl DownloadPayload {
    fn kind(&self) -> DownloadKind {
        match self {
            Self::Iso(_) => DownloadKind::Iso,
            Self::RaspberryPi(_) => DownloadKind::RaspberryPi,
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Iso(release) => release.name.clone(),
            Self::RaspberryPi(image) => image.name.clone(),
        }
    }

    fn expected_size(&self) -> Option<u64> {
        match self {
            Self::Iso(release) => release.size,
            Self::RaspberryPi(image) => image.download_size,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredJob {
    record: DownloadJob,
    payload: DownloadPayload,
    #[serde(default)]
    owner_session: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LedgerEnvelope {
    version: u32,
    jobs: Vec<StoredJob>,
}

#[derive(Debug, Clone)]
pub(crate) struct DownloadLedger {
    root: PathBuf,
}

impl DownloadLedger {
    pub(crate) fn open_default() -> Result<Self> {
        Ok(Self {
            root: state_root()?,
        })
    }

    #[cfg(test)]
    fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn enqueue(&self, payload: DownloadPayload, destination: &Path) -> Result<String> {
        let destination = absolute_destination(destination)?;
        self.update(|jobs| {
            if let Some(existing) = jobs.iter().find(|job| {
                job.record.destination == destination
                    && !job.record.status.is_terminal()
                    && same_payload(&job.payload, &payload)
            }) {
                return Ok(existing.record.id.clone());
            }
            let now = unix_millis()?;
            let mut id = format!("{now:x}-{:x}", std::process::id());
            let mut suffix = 0_u32;
            while jobs.iter().any(|job| job.record.id == id) {
                suffix = suffix.saturating_add(1);
                id = format!("{now:x}-{:x}-{suffix:x}", std::process::id());
            }
            jobs.push(StoredJob {
                record: DownloadJob {
                    id: id.clone(),
                    kind: payload.kind(),
                    label: payload.label(),
                    destination,
                    status: DownloadStatus::Queued,
                    completed: 0,
                    total: payload.expected_size(),
                    message: "Waiting to download".into(),
                    error: None,
                    created_at: now,
                    updated_at: now,
                },
                payload,
                owner_session: None,
            });
            Ok(id)
        })
    }

    pub(crate) fn begin(&self, id: &str) -> Result<(DownloadPayload, PathBuf)> {
        self.update(|jobs| {
            let job = find_job_mut(jobs, id)?;
            if !matches!(
                job.record.status,
                DownloadStatus::Queued
                    | DownloadStatus::Interrupted
                    | DownloadStatus::Failed
                    | DownloadStatus::Cancelled
            ) {
                return Err(Error::DownloadManager(format!(
                    "download {} cannot start while it is {}",
                    job.record.label, job.record.status
                )));
            }
            job.record.status = DownloadStatus::Running;
            job.owner_session = Some(session_id().to_owned());
            job.record.error = None;
            job.record.message = "Preparing secure download".into();
            job.record.updated_at = unix_millis()?;
            Ok((job.payload.clone(), job.record.destination.clone()))
        })
    }

    pub(crate) fn progress(&self, id: &str, progress: &Progress) -> Result<()> {
        self.update(|jobs| {
            let job = find_job_mut(jobs, id)?;
            if job.record.status == DownloadStatus::Cancelled {
                return Ok(());
            }
            job.record.status = DownloadStatus::Running;
            job.record.completed = progress.completed;
            job.record.total = progress.total.or(job.record.total);
            job.record.message = progress.message.clone();
            job.record.updated_at = unix_millis()?;
            Ok(())
        })
    }

    pub(crate) fn pause(&self, id: &str, paused: bool) -> Result<()> {
        self.update(|jobs| {
            let job = find_job_mut(jobs, id)?;
            if matches!(
                job.record.status,
                DownloadStatus::Running | DownloadStatus::Paused
            ) {
                job.record.status = if paused {
                    DownloadStatus::Paused
                } else {
                    DownloadStatus::Running
                };
                job.record.message = if paused {
                    "Paused · partial download preserved".into()
                } else {
                    "Resuming download".into()
                };
                job.record.updated_at = unix_millis()?;
            }
            Ok(())
        })
    }

    pub(crate) fn finish(&self, id: &str, result: std::result::Result<(), &Error>) -> Result<()> {
        self.update(|jobs| {
            let job = find_job_mut(jobs, id)?;
            match result {
                Ok(()) => {
                    job.record.status = DownloadStatus::Completed;
                    job.record.completed = job.record.total.unwrap_or(job.record.completed);
                    job.record.message = "Download verified and ready".into();
                    job.record.error = None;
                }
                Err(Error::OperationCancelled) => {
                    job.record.status = DownloadStatus::Cancelled;
                    job.record.message = "Cancelled · temporary data removed".into();
                    job.record.error = None;
                }
                Err(error) => {
                    job.record.status = DownloadStatus::Failed;
                    job.record.message = "Download stopped · retry is available".into();
                    job.record.error = Some(error.to_string());
                }
            }
            job.owner_session = None;
            job.record.updated_at = unix_millis()?;
            Ok(())
        })
    }

    pub(crate) fn retry(&self, id: &str) -> Result<()> {
        self.update(|jobs| {
            let job = find_job_mut(jobs, id)?;
            if !job.record.status.can_retry() {
                return Err(Error::DownloadManager(format!(
                    "download {} is not retryable while it is {}",
                    job.record.label, job.record.status
                )));
            }
            job.record.status = DownloadStatus::Queued;
            job.owner_session = None;
            job.record.message = "Queued for retry".into();
            job.record.error = None;
            job.record.updated_at = unix_millis()?;
            Ok(())
        })
    }

    pub(crate) fn remove(&self, id: &str) -> Result<()> {
        self.update(|jobs| {
            let position = jobs
                .iter()
                .position(|job| job.record.id == id)
                .ok_or_else(|| {
                    Error::DownloadManager(format!("download job {id} was not found"))
                })?;
            if matches!(
                jobs[position].record.status,
                DownloadStatus::Running | DownloadStatus::Paused
            ) {
                return Err(Error::DownloadManager(
                    "cancel an active download before removing it".into(),
                ));
            }
            let destination = jobs[position].record.destination.clone();
            discard_partial(&destination)?;
            jobs.remove(position);
            Ok(())
        })
    }

    pub(crate) fn list(&self) -> Result<Vec<DownloadJob>> {
        self.update(|jobs| {
            let mut records = jobs
                .iter()
                .map(|job| job.record.clone())
                .collect::<Vec<_>>();
            records.sort_by_key(|record| std::cmp::Reverse(record.updated_at));
            Ok(records)
        })
    }

    pub(crate) fn next_queued(&self) -> Result<Option<DownloadJob>> {
        self.update(|jobs| {
            Ok(jobs
                .iter()
                .filter(|job| job.record.status == DownloadStatus::Queued)
                .min_by_key(|job| job.record.created_at)
                .map(|job| job.record.clone()))
        })
    }

    fn update<T>(&self, change: impl FnOnce(&mut Vec<StoredJob>) -> Result<T>) -> Result<T> {
        fs::create_dir_all(&self.root).map_err(|error| io_error(&self.root, error))?;
        let lock_path = self.root.join("downloads.lock");
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| io_error(&lock_path, error))?;
        FileExt::lock_exclusive(&lock).map_err(|error| io_error(&lock_path, error))?;
        let result = (|| {
            let path = self.root.join("downloads-v1.json");
            let mut jobs = read_ledger(&path)?;
            recover_interrupted(&mut jobs)?;
            let result = change(&mut jobs)?;
            write_ledger(&self.root, &path, &jobs)?;
            Ok(result)
        })();
        let unlock_result = FileExt::unlock(&lock).map_err(|error| io_error(&lock_path, error));
        match (result, unlock_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        }
    }
}

pub(crate) struct ProgressRecorder {
    ledger: DownloadLedger,
    id: String,
    last_saved: Instant,
    last_phase: Option<crate::ProgressPhase>,
}

impl ProgressRecorder {
    pub(crate) fn new(ledger: DownloadLedger, id: String) -> Self {
        Self {
            ledger,
            id,
            last_saved: Instant::now() - PROGRESS_SAVE_INTERVAL,
            last_phase: None,
        }
    }

    pub(crate) fn record(&mut self, progress: &Progress) {
        let phase_changed = self.last_phase.as_ref() != Some(&progress.phase);
        if phase_changed || self.last_saved.elapsed() >= PROGRESS_SAVE_INTERVAL {
            let _ = self.ledger.progress(&self.id, progress);
            self.last_saved = Instant::now();
            self.last_phase = Some(progress.phase.clone());
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PartialMetadata {
    version: u32,
    source: String,
    destination_name: String,
    expected_size: Option<u64>,
    etag: Option<String>,
    last_modified: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TransferProgress {
    pub completed: u64,
    pub total: Option<u64>,
    pub resumed: bool,
    pub resumed_from: u64,
    pub started: Instant,
}

#[derive(Debug)]
pub(crate) struct StagedDownload {
    path: PathBuf,
    metadata_path: PathBuf,
    destination: PathBuf,
    size: u64,
}

impl StagedDownload {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    pub(crate) fn persist(self) -> Result<()> {
        fs::rename(&self.path, &self.destination)
            .map_err(|error| io_error(&self.destination, error))?;
        remove_if_present(&self.metadata_path)?;
        sync_parent(&self.destination)
    }

    pub(crate) fn discard(self) -> Result<()> {
        remove_if_present(&self.path)?;
        remove_if_present(&self.metadata_path)
    }
}

pub(crate) fn stage(
    client: &Client,
    source: &Url,
    destination: &Path,
    expected_size: Option<u64>,
    control: &OperationControl,
    mut on_progress: impl FnMut(TransferProgress),
) -> Result<StagedDownload> {
    if destination.exists() {
        return Err(Error::InvalidDownload(format!(
            "{} already exists",
            destination.display()
        )));
    }
    let (part_path, metadata_path) = partial_paths(destination)?;
    let mut metadata = load_or_initialize_partial(
        &metadata_path,
        &part_path,
        source,
        destination,
        expected_size,
    )?;
    let result = transfer_with_reconnects(
        client,
        source,
        &part_path,
        &metadata_path,
        &mut metadata,
        control,
        &mut on_progress,
    );
    match result {
        Ok(size) => Ok(StagedDownload {
            path: part_path,
            metadata_path,
            destination: destination.to_owned(),
            size,
        }),
        Err(Error::OperationCancelled) => {
            discard_partial(destination)?;
            Err(Error::OperationCancelled)
        }
        Err(error) => Err(error),
    }
}

fn transfer_with_reconnects(
    client: &Client,
    source: &Url,
    part_path: &Path,
    metadata_path: &Path,
    metadata: &mut PartialMetadata,
    control: &OperationControl,
    on_progress: &mut impl FnMut(TransferProgress),
) -> Result<u64> {
    let started = Instant::now();
    let resumed_from = file_length(part_path)?;
    let mut reconnects = 0_usize;
    loop {
        control.checkpoint()?;
        match transfer_once(
            client,
            source,
            part_path,
            metadata_path,
            metadata,
            control,
            started,
            resumed_from,
            on_progress,
        ) {
            Ok(size) => return Ok(size),
            Err(error @ (Error::OperationCancelled | Error::InvalidDownload(_))) => {
                return Err(error);
            }
            Err(error) if reconnects >= MAX_RECONNECTS => return Err(error),
            Err(_) => {
                reconnects += 1;
                std::thread::sleep(Duration::from_millis(250 * reconnects as u64));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn transfer_once(
    client: &Client,
    source: &Url,
    part_path: &Path,
    metadata_path: &Path,
    metadata: &mut PartialMetadata,
    control: &OperationControl,
    started: Instant,
    resumed_from: u64,
    on_progress: &mut impl FnMut(TransferProgress),
) -> Result<u64> {
    let existing = file_length(part_path)?;
    let mut request = client.get(source.clone());
    if existing > 0 {
        request = request.header(RANGE, format!("bytes={existing}-"));
        if let Some(validator) = metadata.etag.as_ref().or(metadata.last_modified.as_ref()) {
            request = request.header(IF_RANGE, validator);
        }
    }
    let mut response = request
        .send()
        .map_err(|error| network_error(source, error))?;
    validate_secure_redirect(source, &response)?;
    if response.status() == StatusCode::RANGE_NOT_SATISFIABLE
        && metadata.expected_size == Some(existing)
    {
        return Ok(existing);
    }
    if !response.status().is_success() {
        return Err(network_error(source, format!("HTTP {}", response.status())));
    }

    let resumed = existing > 0 && response.status() == StatusCode::PARTIAL_CONTENT;
    let offset = if resumed {
        validate_content_range(&response, existing)?
    } else {
        0
    };
    let total = response_total(&response, offset).or(metadata.expected_size);
    if let (Some(expected), Some(actual)) = (metadata.expected_size, total)
        && expected != actual
    {
        return Err(Error::InvalidDownload(format!(
            "download reports {actual} bytes; catalog expected {expected}"
        )));
    }
    metadata.expected_size = total.or(metadata.expected_size);
    metadata.etag = header_text(&response, ETAG);
    metadata.last_modified = header_text(&response, LAST_MODIFIED);
    write_partial_metadata(metadata_path, metadata)?;

    let mut output = open_partial(part_path, resumed)?;
    if resumed {
        output
            .seek(SeekFrom::Start(offset))
            .map_err(|error| io_error(part_path, error))?;
    }
    let mut completed = offset;
    on_progress(TransferProgress {
        completed,
        total,
        resumed,
        resumed_from,
        started,
    });
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        control.checkpoint()?;
        let count = response
            .read(&mut buffer)
            .map_err(|error| network_error(source, error))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| io_error(part_path, error))?;
        completed = completed.saturating_add(count as u64);
        if total.is_some_and(|total| completed > total) {
            return Err(Error::InvalidDownload(
                "download exceeded its advertised size".into(),
            ));
        }
        on_progress(TransferProgress {
            completed,
            total,
            resumed,
            resumed_from,
            started,
        });
    }
    if let Some(expected) = total
        && expected != completed
    {
        return Err(Error::Network {
            url: source.to_string(),
            message: format!("transfer stopped at {completed} of {expected} bytes"),
        });
    }
    output
        .sync_all()
        .map_err(|error| io_error(part_path, error))?;
    Ok(completed)
}

pub(crate) fn transfer_message(prefix: &str, progress: TransferProgress) -> String {
    let transferred_this_run = progress.completed.saturating_sub(progress.resumed_from);
    let elapsed = progress.started.elapsed().as_secs_f64().max(0.001);
    let bytes_per_second = transferred_this_run as f64 / elapsed;
    let speed = format!("{}/s", crate::format_bytes(bytes_per_second as u64));
    let resume = if progress.resumed { " · resumed" } else { "" };
    match progress.total.filter(|total| *total > 0) {
        Some(total) => {
            let percent = progress.completed as f64 * 100. / total as f64;
            let remaining =
                total.saturating_sub(progress.completed) as f64 / bytes_per_second.max(1.);
            format!(
                "{prefix}{resume} · {} / {} · {percent:.1}% · {speed} · ETA {}",
                crate::format_bytes(progress.completed),
                crate::format_bytes(total),
                format_eta(remaining)
            )
        }
        None => format!(
            "{prefix}{resume} · {} transferred · {speed}",
            crate::format_bytes(progress.completed)
        ),
    }
}

pub(crate) fn discard_partial(destination: &Path) -> Result<()> {
    let (part, metadata) = partial_paths(destination)?;
    remove_if_present(&part)?;
    remove_if_present(&metadata)
}

fn same_payload(left: &DownloadPayload, right: &DownloadPayload) -> bool {
    match (left, right) {
        (DownloadPayload::Iso(left), DownloadPayload::Iso(right)) => left.url == right.url,
        (DownloadPayload::RaspberryPi(left), DownloadPayload::RaspberryPi(right)) => {
            left.download_url == right.download_url
        }
        _ => false,
    }
}

fn find_job_mut<'a>(jobs: &'a mut [StoredJob], id: &str) -> Result<&'a mut StoredJob> {
    jobs.iter_mut()
        .find(|job| job.record.id == id)
        .ok_or_else(|| Error::DownloadManager(format!("download job {id} was not found")))
}

fn recover_interrupted(jobs: &mut [StoredJob]) -> Result<()> {
    let now = unix_millis()?;
    for job in jobs.iter_mut().filter(|job| {
        matches!(
            job.record.status,
            DownloadStatus::Running | DownloadStatus::Paused
        ) && job.owner_session.as_deref() != Some(session_id())
            && now.saturating_sub(job.record.updated_at) >= ACTIVE_LEASE_MILLIS
    }) {
        job.record.status = DownloadStatus::Interrupted;
        job.owner_session = None;
        job.record.message = "Interrupted · retry will resume the partial download".into();
        job.record.updated_at = now;
    }
    Ok(())
}

fn session_id() -> &'static str {
    static SESSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SESSION
        .get_or_init(|| {
            format!(
                "{:x}-{:x}",
                unix_millis().unwrap_or_default(),
                std::process::id()
            )
        })
        .as_str()
}

fn read_ledger(path: &Path) -> Result<Vec<StoredJob>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(path, error)),
    };
    let envelope: LedgerEnvelope = serde_json::from_slice(&bytes).map_err(|error| {
        Error::DownloadManager(format!(
            "invalid download history at {}: {error}",
            path.display()
        ))
    })?;
    if envelope.version != LEDGER_VERSION {
        return Err(Error::DownloadManager(format!(
            "unsupported download history version {}",
            envelope.version
        )));
    }
    Ok(envelope.jobs)
}

fn write_ledger(root: &Path, path: &Path, jobs: &[StoredJob]) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(&LedgerEnvelope {
        version: LEDGER_VERSION,
        jobs: jobs.to_vec(),
    })
    .map_err(|error| Error::DownloadManager(format!("serialize download history: {error}")))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".downloads-")
        .suffix(".tmp")
        .tempfile_in(root)
        .map_err(|error| io_error(root, error))?;
    temporary
        .write_all(&bytes)
        .map_err(|error| io_error(temporary.path(), error))?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|error| io_error(temporary.path(), error))?;
    temporary
        .persist(path)
        .map_err(|error| io_error(path, error.error))?;
    Ok(())
}

fn state_root() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("BOOTABLE_STATE_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    #[cfg(target_os = "windows")]
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(root).join("Bootable"));
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("Bootable"));
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(root) = std::env::var_os("XDG_STATE_HOME") {
            return Ok(PathBuf::from(root).join("bootable"));
        }
        if let Some(home) = std::env::var_os("HOME") {
            return Ok(PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("bootable"));
        }
    }
    Err(Error::DownloadManager(
        "could not determine a persistent state directory".into(),
    ))
}

fn absolute_destination(destination: &Path) -> Result<PathBuf> {
    if destination.is_absolute() {
        return Ok(destination.to_owned());
    }
    std::env::current_dir()
        .map(|directory| directory.join(destination))
        .map_err(|error| io_error(destination, error))
}

fn unix_millis() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .map_err(|error| Error::DownloadManager(format!("system clock before Unix epoch: {error}")))
}

fn partial_paths(destination: &Path) -> Result<(PathBuf, PathBuf)> {
    let parent = destination.parent().ok_or_else(|| {
        Error::InvalidDownload("download destination has no parent directory".into())
    })?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::InvalidDownload("download destination has no file name".into()))?;
    Ok((
        parent.join(format!(".bootable-{file_name}.part")),
        parent.join(format!(".bootable-{file_name}.part.json")),
    ))
}

fn load_or_initialize_partial(
    metadata_path: &Path,
    part_path: &Path,
    source: &Url,
    destination: &Path,
    expected_size: Option<u64>,
) -> Result<PartialMetadata> {
    let destination_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    match fs::read(metadata_path) {
        Ok(bytes) => {
            let metadata: PartialMetadata = serde_json::from_slice(&bytes).map_err(|error| {
                Error::InvalidDownload(format!(
                    "partial download metadata at {} is invalid: {error}",
                    metadata_path.display()
                ))
            })?;
            if metadata.version != PARTIAL_VERSION
                || metadata.source != source.as_str()
                || metadata.destination_name != destination_name
            {
                return Err(Error::InvalidDownload(format!(
                    "partial download at {} belongs to another source; remove it explicitly before retrying",
                    part_path.display()
                )));
            }
            Ok(metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if part_path.exists() {
                return Err(Error::InvalidDownload(format!(
                    "unrecognized partial file at {}; refusing to overwrite it",
                    part_path.display()
                )));
            }
            let metadata = PartialMetadata {
                version: PARTIAL_VERSION,
                source: source.to_string(),
                destination_name,
                expected_size,
                etag: None,
                last_modified: None,
            };
            write_partial_metadata(metadata_path, &metadata)?;
            Ok(metadata)
        }
        Err(error) => Err(io_error(metadata_path, error)),
    }
}

fn write_partial_metadata(path: &Path, metadata: &PartialMetadata) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidDownload("partial metadata has no parent directory".into()))?;
    let bytes = serde_json::to_vec(metadata)
        .map_err(|error| Error::DownloadManager(format!("serialize partial state: {error}")))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".bootable-transfer-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| io_error(parent, error))?;
    temporary
        .write_all(&bytes)
        .map_err(|error| io_error(temporary.path(), error))?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|error| io_error(temporary.path(), error))?;
    temporary
        .persist(path)
        .map_err(|error| io_error(path, error.error))?;
    Ok(())
}

fn open_partial(path: &Path, append: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true);
    if append {
        options.append(true);
    } else {
        options.truncate(true);
    }
    options.open(path).map_err(|error| io_error(path, error))
}

fn file_length(path: &Path) -> Result<u64> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(metadata.len()),
        Ok(_) => Err(Error::InvalidDownload(format!(
            "partial download path {} is not a regular file",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(io_error(path, error)),
    }
}

fn validate_secure_redirect(source: &Url, response: &Response) -> Result<()> {
    if source.scheme() == "https" && response.url().scheme() != "https" {
        return Err(Error::InvalidDownload(
            "download redirected to an insecure URL".into(),
        ));
    }
    Ok(())
}

fn validate_content_range(response: &Response, expected_start: u64) -> Result<u64> {
    let value = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| Error::InvalidDownload("resume response omitted Content-Range".into()))?;
    let start = value
        .strip_prefix("bytes ")
        .and_then(|value| value.split_once('-'))
        .and_then(|(start, _)| start.parse::<u64>().ok())
        .ok_or_else(|| {
            Error::InvalidDownload("resume response has invalid Content-Range".into())
        })?;
    if start != expected_start {
        return Err(Error::InvalidDownload(format!(
            "resume response began at byte {start}; expected {expected_start}"
        )));
    }
    Ok(start)
}

fn response_total(response: &Response, offset: u64) -> Option<u64> {
    response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit_once('/'))
        .and_then(|(_, total)| total.parse::<u64>().ok())
        .or_else(|| response.content_length().map(|length| offset + length))
}

fn header_text(response: &Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(parent, error))
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> Result<()> {
    Ok(())
}

fn format_eta(seconds: f64) -> String {
    let seconds = seconds.max(0.) as u64;
    if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn network_error(url: &Url, error: impl std::fmt::Display) -> Error {
    Error::Network {
        url: url.to_string(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn release(url: &str) -> DownloadPayload {
        DownloadPayload::Iso(IsoRelease {
            name: "test.iso".into(),
            url: url.into(),
            size: Some(8),
            published: None,
            checksum_algorithm: None,
            checksum: None,
            checksum_url: None,
        })
    }

    #[test]
    fn ledger_recovers_running_jobs_and_supports_retry() {
        let directory = tempfile::tempdir().expect("state");
        let ledger = DownloadLedger::at(directory.path().to_owned());
        let destination = directory.path().join("test.iso");
        let id = ledger
            .enqueue(release("https://example.com/test.iso"), &destination)
            .expect("enqueue");
        ledger.begin(&id).expect("begin");
        ledger
            .update(|jobs| {
                let job = find_job_mut(jobs, &id)?;
                job.owner_session = Some("previous-process".into());
                job.record.updated_at = 0;
                Ok(())
            })
            .expect("simulate interrupted process");

        let recovered = ledger.list().expect("list");
        assert_eq!(recovered[0].status, DownloadStatus::Interrupted);
        assert!(recovered[0].message.contains("resume"));
        ledger.retry(&id).expect("retry");
        assert_eq!(
            ledger.list().expect("list")[0].status,
            DownloadStatus::Queued
        );
    }

    #[test]
    fn ledger_deduplicates_an_active_destination_and_source() {
        let directory = tempfile::tempdir().expect("state");
        let ledger = DownloadLedger::at(directory.path().to_owned());
        let destination = directory.path().join("test.iso");
        let first = ledger
            .enqueue(release("https://example.com/test.iso"), &destination)
            .expect("first");
        let second = ledger
            .enqueue(release("https://example.com/test.iso"), &destination)
            .expect("second");
        assert_eq!(first, second);
        assert_eq!(ledger.list().expect("list").len(), 1);
    }

    #[test]
    fn queue_is_fifo_and_removing_history_keeps_completed_image() {
        let directory = tempfile::tempdir().expect("state");
        let ledger = DownloadLedger::at(directory.path().join("state"));
        let first_destination = directory.path().join("first.iso");
        let second_destination = directory.path().join("second.iso");
        let first = ledger
            .enqueue(release("https://example.com/first.iso"), &first_destination)
            .expect("first");
        let second = ledger
            .enqueue(
                release("https://example.com/second.iso"),
                &second_destination,
            )
            .expect("second");
        assert_eq!(ledger.next_queued().expect("next").expect("job").id, first);

        ledger.begin(&first).expect("begin first");
        ledger.finish(&first, Ok(())).expect("finish first");
        fs::write(&first_destination, b"complete image").expect("completed image");
        assert_eq!(ledger.next_queued().expect("next").expect("job").id, second);
        ledger.remove(&first).expect("remove history");
        assert_eq!(
            fs::read(first_destination).expect("completed image remains"),
            b"complete image"
        );
    }

    #[test]
    fn refuses_an_unowned_partial_file() {
        let directory = tempfile::tempdir().expect("directory");
        let destination = directory.path().join("system.iso");
        let (partial, metadata) = partial_paths(&destination).expect("partial paths");
        fs::write(&partial, b"someone else's data").expect("partial");
        let source = Url::parse("https://example.com/system.iso").expect("URL");
        let error =
            load_or_initialize_partial(&metadata, &partial, &source, &destination, Some(10))
                .expect_err("must refuse unknown partial");
        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(
            fs::read(partial).expect("preserved"),
            b"someone else's data"
        );
    }

    #[test]
    fn resumes_an_owned_partial_with_a_valid_range_response() {
        let directory = tempfile::tempdir().expect("directory");
        let destination = directory.path().join("system.iso");
        let Some((url, server)) = one_response_server(|request| {
            assert!(request.to_ascii_lowercase().contains("range: bytes=4-"));
            concat!(
                "HTTP/1.1 206 Partial Content\r\n",
                "Content-Length: 4\r\n",
                "Content-Range: bytes 4-7/8\r\n",
                "ETag: \"v1\"\r\n",
                "Connection: close\r\n\r\n",
                "EFGH"
            )
            .as_bytes()
            .to_vec()
        }) else {
            return;
        };
        let source = Url::parse(&url).expect("URL");
        let (partial, metadata) = partial_paths(&destination).expect("partial paths");
        load_or_initialize_partial(&metadata, &partial, &source, &destination, Some(8))
            .expect("metadata");
        fs::write(&partial, b"ABCD").expect("partial");

        let mut updates = Vec::new();
        let staged = stage(
            &Client::new(),
            &source,
            &destination,
            Some(8),
            &OperationControl::new(),
            |progress| updates.push(progress),
        )
        .expect("resume");
        server.join().expect("server");
        assert_eq!(fs::read(staged.path()).expect("staged"), b"ABCDEFGH");
        assert!(updates.iter().any(|progress| progress.resumed));
        staged.persist().expect("persist");
        assert_eq!(fs::read(destination).expect("destination"), b"ABCDEFGH");
    }

    #[test]
    fn restarts_safely_when_the_server_ignores_range() {
        let directory = tempfile::tempdir().expect("directory");
        let destination = directory.path().join("system.iso");
        let Some((url, server)) = one_response_server(|request| {
            assert!(request.to_ascii_lowercase().contains("range: bytes=4-"));
            concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Length: 8\r\n",
                "Connection: close\r\n\r\n",
                "ABCDEFGH"
            )
            .as_bytes()
            .to_vec()
        }) else {
            return;
        };
        let source = Url::parse(&url).expect("URL");
        let (partial, metadata) = partial_paths(&destination).expect("partial paths");
        load_or_initialize_partial(&metadata, &partial, &source, &destination, Some(8))
            .expect("metadata");
        fs::write(&partial, b"ABCD").expect("partial");

        let staged = stage(
            &Client::new(),
            &source,
            &destination,
            Some(8),
            &OperationControl::new(),
            |_| {},
        )
        .expect("restart");
        server.join().expect("server");
        assert_eq!(fs::read(staged.path()).expect("staged"), b"ABCDEFGH");
    }

    #[test]
    fn explicit_cancellation_removes_owned_partial_state() {
        let directory = tempfile::tempdir().expect("directory");
        let destination = directory.path().join("system.iso");
        let source = Url::parse("https://example.com/system.iso").expect("URL");
        let (partial, metadata) = partial_paths(&destination).expect("partial paths");
        load_or_initialize_partial(&metadata, &partial, &source, &destination, Some(8))
            .expect("metadata");
        fs::write(&partial, b"ABCD").expect("partial");
        let control = OperationControl::new();
        control.cancel();

        let error = stage(
            &Client::new(),
            &source,
            &destination,
            Some(8),
            &control,
            |_| {},
        )
        .expect_err("cancelled");
        assert!(matches!(error, Error::OperationCancelled));
        assert!(!partial.exists());
        assert!(!metadata.exists());
    }

    fn one_response_server(
        response: impl FnOnce(&str) -> Vec<u8> + Send + 'static,
    ) -> Option<(String, thread::JoinHandle<()>)> {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return None,
            Err(error) => panic!("bind test server: {error}"),
        };
        let address = listener.local_addr().expect("address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("timeout");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let request = String::from_utf8(request).expect("HTTP request");
            stream
                .write_all(&response(&request))
                .expect("write response");
        });
        Some((format!("http://{address}/system.iso"), handle))
    }
}
