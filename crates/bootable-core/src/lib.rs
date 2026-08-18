mod catalog;
mod catalog_cache;
mod checksum;
mod download;
mod error;
mod inspect;
mod model;
mod operation;
mod pi_catalog;
mod plan;
mod platform;
#[cfg(target_os = "linux")]
mod privilege;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(test, allow(dead_code))]
mod privilege_macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(test, allow(dead_code))]
mod privilege_windows;
mod windows;

use std::path::Path;

pub use catalog::{DistributionBundle, DistributionDetails, DistributionSummary, IsoRelease};
pub use catalog_cache::{CacheMode, CatalogFetch, CatalogOrigin, CatalogState};
pub use checksum::{Checksum, ChecksumAlgorithm};
pub use download::{DownloadJob, DownloadKind, DownloadStatus};
pub use error::{Error, Result};
pub use model::{
    BadBlockCheck, CompressedImageKind, Device, DeviceId, ImageCompression, ImageKind, ImageReport,
    MountPoint, PlanStep, PrivilegedWriteCommand, PrivilegedWriteEvent, PrivilegedWriteRequest,
    Progress, ProgressPhase, ReviewReadiness, WindowsExperienceOptions, WindowsPartitionScheme,
    WindowsPayload, WindowsRegionalOptions, WriteOptions, WritePlan, WriteStrategy,
    destructive_confirmation_ready, format_bytes, review_readiness,
};
pub use operation::{OperationControl, OperationState};
pub use pi_catalog::{PiCatalog, PiDevice, PiImage};
pub use windows::{host_regional_options, suggested_account_name};

use platform::NativePlatform;

pub struct Bootable {
    platform: NativePlatform,
}

impl Default for Bootable {
    fn default() -> Self {
        Self::native()
    }
}

impl Bootable {
    pub fn native() -> Self {
        Self {
            platform: NativePlatform::new(),
        }
    }

    pub fn discover_devices(&self) -> Result<Vec<Device>> {
        self.platform.devices()
    }

    pub fn inspect_image(&self, path: impl AsRef<Path>) -> Result<ImageReport> {
        let path = path.as_ref();
        match self.platform.inspect_override(path) {
            Some(result) => result,
            None => inspect::inspect(path),
        }
    }

    pub fn checksum_image(
        &self,
        path: impl AsRef<Path>,
        algorithm: ChecksumAlgorithm,
    ) -> Result<Checksum> {
        checksum::compute(path.as_ref(), algorithm)
    }

    pub fn popular_distributions(&self, limit: usize) -> Result<Vec<DistributionSummary>> {
        catalog::popular_distributions(limit)
    }

    pub fn popular_distributions_cached(
        &self,
        limit: usize,
        mode: CacheMode,
    ) -> Result<CatalogFetch<Vec<DistributionSummary>>> {
        catalog_cache::load_or_fetch(&format!("popular-{limit}"), mode, || {
            catalog::popular_distributions(limit)
        })
    }

    pub fn distribution_directory(&self) -> Result<Vec<DistributionSummary>> {
        catalog::distribution_directory()
    }

    pub fn distribution_directory_cached(
        &self,
        mode: CacheMode,
    ) -> Result<CatalogFetch<Vec<DistributionSummary>>> {
        catalog_cache::load_or_fetch("directory", mode, catalog::distribution_directory)
    }

    pub fn distributions_based_on(&self, base: &str) -> Result<Vec<DistributionSummary>> {
        catalog::distributions_based_on(base)
    }

    pub fn distributions_based_on_cached(
        &self,
        base: &str,
        mode: CacheMode,
    ) -> Result<CatalogFetch<Vec<DistributionSummary>>> {
        let key = match base {
            "Arch" => "based-on-arch-ranked-v2",
            "Debian" => "based-on-debian-ranked-v2",
            _ => {
                return Err(Error::InvalidCatalog(
                    "quick base search supports Arch or Debian".into(),
                ));
            }
        };
        catalog_cache::load_or_fetch(key, mode, || catalog::distributions_based_on(base))
    }

    pub fn distribution_details(&self, slug: &str) -> Result<DistributionDetails> {
        catalog::distribution_details(slug)
    }

    pub fn catalog_artwork(&self, url: &str) -> Result<Vec<u8>> {
        catalog::artwork(url)
    }

    pub fn distribution_bundle(&self, slug: &str) -> Result<DistributionBundle> {
        catalog::distribution_bundle(slug)
    }

    pub fn distribution_bundle_cached(
        &self,
        slug: &str,
        mode: CacheMode,
    ) -> Result<CatalogFetch<DistributionBundle>> {
        let key = format!("distribution-{slug}");
        catalog_cache::load_or_fetch(&key, mode, || catalog::distribution_bundle(slug))
    }

    pub fn iso_releases(&self, source_url: &str) -> Result<Vec<IsoRelease>> {
        catalog::iso_releases(source_url)
    }

    pub fn download_iso(
        &self,
        release: &IsoRelease,
        destination: impl AsRef<Path>,
        mut progress: impl FnMut(Progress),
    ) -> Result<ImageReport> {
        self.download_iso_controlled(
            release,
            destination,
            &OperationControl::new(),
            &mut progress,
        )
    }

    pub fn download_iso_controlled(
        &self,
        release: &IsoRelease,
        destination: impl AsRef<Path>,
        control: &OperationControl,
        progress: impl FnMut(Progress),
    ) -> Result<ImageReport> {
        let id = self.enqueue_iso_download(release, destination)?;
        self.run_download_job(&id, control, progress)
    }

    pub fn enqueue_iso_download(
        &self,
        release: &IsoRelease,
        destination: impl AsRef<Path>,
    ) -> Result<String> {
        download::DownloadLedger::open_default()?.enqueue(
            download::DownloadPayload::Iso(release.clone()),
            destination.as_ref(),
        )
    }

    fn download_iso_payload(
        &self,
        release: &IsoRelease,
        destination: &Path,
        control: &OperationControl,
        mut progress: impl FnMut(Progress),
    ) -> Result<ImageReport> {
        catalog::download_iso(release, destination, control, &mut progress)?;
        let publisher_checksum = release.checksum.is_some() || release.checksum_url.is_some();
        control.checkpoint()?;
        progress(Progress {
            phase: ProgressPhase::Verifying,
            completed: 0,
            total: None,
            message: "Stage 5/5 · Inspecting boot structure and media strategy".into(),
        });
        let report = self.inspect_image(destination)?;
        progress(Progress {
            phase: ProgressPhase::Finished,
            completed: report.size,
            total: Some(report.size),
            message: if publisher_checksum {
                format!(
                    "Ready · publisher checksum verified · {}",
                    report.path.display()
                )
            } else {
                format!(
                    "Ready · HTTPS transfer and boot structure checked · publisher checksum unavailable · {}",
                    report.path.display()
                )
            },
        });
        Ok(report)
    }

    pub fn raspberry_pi_catalog(&self) -> Result<PiCatalog> {
        pi_catalog::catalog()
    }

    pub fn raspberry_pi_catalog_cached(&self, mode: CacheMode) -> Result<CatalogFetch<PiCatalog>> {
        catalog_cache::load_or_fetch("raspberry-pi", mode, pi_catalog::catalog)
    }

    pub fn download_pi_image(
        &self,
        image: &PiImage,
        destination: impl AsRef<Path>,
        mut progress: impl FnMut(Progress),
    ) -> Result<ImageReport> {
        self.download_pi_image_controlled(
            image,
            destination,
            &OperationControl::new(),
            &mut progress,
        )
    }

    pub fn download_pi_image_controlled(
        &self,
        image: &PiImage,
        destination: impl AsRef<Path>,
        control: &OperationControl,
        progress: impl FnMut(Progress),
    ) -> Result<ImageReport> {
        let id = self.enqueue_pi_download(image, destination)?;
        self.run_download_job(&id, control, progress)
    }

    pub fn enqueue_pi_download(
        &self,
        image: &PiImage,
        destination: impl AsRef<Path>,
    ) -> Result<String> {
        download::DownloadLedger::open_default()?.enqueue(
            download::DownloadPayload::RaspberryPi(image.clone()),
            destination.as_ref(),
        )
    }

    fn download_pi_payload(
        &self,
        image: &PiImage,
        destination: &Path,
        control: &OperationControl,
        mut progress: impl FnMut(Progress),
    ) -> Result<ImageReport> {
        pi_catalog::download_image(image, destination, control, &mut progress)?;
        let publisher_checksum =
            image.download_sha256.is_some() || image.extracted_sha256.is_some();
        control.checkpoint()?;
        progress(Progress {
            phase: ProgressPhase::Verifying,
            completed: 0,
            total: None,
            message: "Stage 6/6 · Inspecting boot structure and media strategy".into(),
        });
        let report = self.inspect_image(destination)?;
        progress(Progress {
            phase: ProgressPhase::Finished,
            completed: report.size,
            total: Some(report.size),
            message: if publisher_checksum {
                format!(
                    "Ready · publisher checksum verified · {}",
                    report.path.display()
                )
            } else {
                format!(
                    "Ready · transfer sizes and boot structure checked · publisher checksum unavailable · {}",
                    report.path.display()
                )
            },
        });
        Ok(report)
    }

    pub fn download_jobs(&self) -> Result<Vec<DownloadJob>> {
        download::DownloadLedger::open_default()?.list()
    }

    pub fn next_queued_download(&self) -> Result<Option<DownloadJob>> {
        download::DownloadLedger::open_default()?.next_queued()
    }

    pub fn set_download_paused(&self, id: &str, paused: bool) -> Result<()> {
        download::DownloadLedger::open_default()?.pause(id, paused)
    }

    pub fn remove_download_job(&self, id: &str) -> Result<()> {
        download::DownloadLedger::open_default()?.remove(id)
    }

    pub fn retry_download_job(
        &self,
        id: &str,
        control: &OperationControl,
        progress: impl FnMut(Progress),
    ) -> Result<ImageReport> {
        download::DownloadLedger::open_default()?.retry(id)?;
        self.run_download_job(id, control, progress)
    }

    pub fn queue_download_retry(&self, id: &str) -> Result<()> {
        download::DownloadLedger::open_default()?.retry(id)
    }

    pub fn run_download_job(
        &self,
        id: &str,
        control: &OperationControl,
        mut progress: impl FnMut(Progress),
    ) -> Result<ImageReport> {
        let ledger = download::DownloadLedger::open_default()?;
        let (payload, destination) = ledger.begin(id)?;
        let mut recorder = download::ProgressRecorder::new(ledger.clone(), id.to_owned());
        let result = match payload {
            download::DownloadPayload::Iso(release) => {
                self.download_iso_payload(&release, &destination, control, |update| {
                    recorder.record(&update);
                    progress(update);
                })
            }
            download::DownloadPayload::RaspberryPi(image) => {
                self.download_pi_payload(&image, &destination, control, |update| {
                    recorder.record(&update);
                    progress(update);
                })
            }
        };
        let ledger_result = ledger.finish(id, result.as_ref().map(|_| ()));
        match (result, ledger_result) {
            (Ok(report), Ok(())) => Ok(report),
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        }
    }

    pub fn plan(&self, image: ImageReport, target: Device) -> Result<WritePlan> {
        plan::build(image, target)
    }

    pub fn plan_with_options(
        &self,
        image: ImageReport,
        target: Device,
        options: WriteOptions,
    ) -> Result<WritePlan> {
        plan::build_with_options(image, target, options)
    }

    pub fn prepare(
        &self,
        image_path: impl AsRef<Path>,
        target_id_or_path: &str,
    ) -> Result<WritePlan> {
        let image = self.inspect_image(image_path)?;
        let target = self
            .discover_devices()?
            .into_iter()
            .find(|device| {
                device.id.as_str() == target_id_or_path
                    || device.path.to_string_lossy() == target_id_or_path
            })
            .ok_or_else(|| Error::DeviceNotFound(target_id_or_path.into()))?;
        self.plan(image, target)
    }

    pub fn prepare_with_options(
        &self,
        image_path: impl AsRef<Path>,
        target_id_or_path: &str,
        options: WriteOptions,
    ) -> Result<WritePlan> {
        let image = self.inspect_image(image_path)?;
        let target = self
            .discover_devices()?
            .into_iter()
            .find(|device| {
                device.id.as_str() == target_id_or_path
                    || device.path.to_string_lossy() == target_id_or_path
            })
            .ok_or_else(|| Error::DeviceNotFound(target_id_or_path.into()))?;
        self.plan_with_options(image, target, options)
    }

    pub fn write(
        &self,
        plan: &WritePlan,
        confirmation: &str,
        mut progress: impl FnMut(Progress),
    ) -> Result<()> {
        self.platform
            .write(plan, confirmation, &OperationControl::new(), &mut progress)
    }

    pub fn write_controlled(
        &self,
        plan: &WritePlan,
        confirmation: &str,
        control: &OperationControl,
        mut progress: impl FnMut(Progress),
    ) -> Result<()> {
        self.platform
            .write(plan, confirmation, control, &mut progress)
    }

    pub fn write_with_privilege(
        &self,
        plan: &WritePlan,
        confirmation: &str,
        mut progress: impl FnMut(Progress),
    ) -> Result<()> {
        self.write_with_privilege_controlled(
            plan,
            confirmation,
            &OperationControl::new(),
            &mut progress,
        )
    }

    pub fn write_with_privilege_controlled(
        &self,
        plan: &WritePlan,
        confirmation: &str,
        control: &OperationControl,
        mut progress: impl FnMut(Progress),
    ) -> Result<()> {
        match self.write_controlled(plan, confirmation, control, &mut progress) {
            Err(Error::NotPrivileged) => {
                #[cfg(target_os = "linux")]
                {
                    privilege::write_via_pkexec(plan, confirmation, control, &mut progress)
                }
                #[cfg(target_os = "macos")]
                {
                    privilege_macos::write_via_authorization(
                        plan,
                        confirmation,
                        control,
                        &mut progress,
                    )
                }
                #[cfg(target_os = "windows")]
                {
                    privilege_windows::write_via_uac(plan, confirmation, control, &mut progress)
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
                {
                    Err(Error::NotPrivileged)
                }
            }
            result => result,
        }
    }

    pub fn backup_device(
        &self,
        device_id_or_path: &str,
        destination: impl AsRef<Path>,
        mut progress: impl FnMut(Progress),
    ) -> Result<()> {
        self.platform
            .backup(device_id_or_path, destination.as_ref(), &mut progress)
    }
}
