use std::fmt;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bootable_core::{
    BadBlockCheck, Bootable, CacheMode, CatalogFacet, CatalogFetch, CatalogState,
    ChecksumAlgorithm, Device, DiscoverySession, DiscoverySource, DistributionBundle,
    DistributionDetails, DistributionSummary, DownloadCompletion, DownloadLaunch, DownloadRequest,
    DownloadStatus, ImageReport, IntegrityState, IsoRelease, Locale, ManagedDownloadSession,
    Message, OperationControl, OperationState, PiCatalog, Preferences, Progress, ProgressPhase,
    QuickAccess, ReviewReadiness, ReviewedWriteSession, Strings, WorkspaceProgress,
    WorkspaceStepState, WriteCompletion, WriteOptions, WritePlan, catalog_search_summary,
    device_details_in, distribution_matches_query, format_bytes, help_intro, help_sections,
    removable_media_status_in, review_readiness, target_eligibility_label,
    target_eligibility_label_in, workspace_progress,
};
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui_image::{Resize, StatefulImage, picker::Picker, protocol::StatefulProtocol};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const DEVICE_SCAN_INTERVAL: Duration = Duration::from_secs(1);
const DOWNLOAD_SCAN_INTERVAL: Duration = Duration::from_secs(5);
const BG: Color = Color::Rgb(11, 17, 25);
const PANEL: Color = Color::Rgb(17, 25, 35);
const PANEL_SOFT: Color = Color::Rgb(13, 21, 31);
const BORDER: Color = Color::Rgb(36, 50, 68);
const MUTED: Color = Color::Rgb(143, 164, 189);
const ACCENT: Color = Color::Rgb(91, 215, 192);

#[derive(Debug, Parser)]
#[command(
    name = "bootable",
    version,
    about = "Inspect, plan, and safely write boot media"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(long, value_name = "IMAGE", global = true)]
    image: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Show popular distributions from DistroWatch.
    Catalog {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Resolve current ISO downloads for a DistroWatch distribution slug.
    Releases {
        slug: String,
        #[arg(long)]
        json: bool,
    },
    /// Download, verify, and inspect an ISO from the catalog.
    Download {
        slug: String,
        #[arg(long, default_value_t = 0)]
        index: usize,
        #[arg(long, value_name = "ISO_FILE")]
        output: Option<PathBuf>,
        /// Emit newline-delimited JSON progress for graphical clients.
        #[arg(long)]
        json_progress: bool,
        /// Refuse the download unless the publisher's checksum manifest carries
        /// a verified signature from a key Bootable pins (exit 4 otherwise).
        #[arg(long)]
        require_signature: bool,
    },
    /// List official Raspberry Pi Imager images.
    PiImages {
        #[arg(long)]
        device: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Download, verify, extract, and inspect a Raspberry Pi image.
    PiDownload {
        index: usize,
        #[arg(long, value_name = "IMG_FILE")]
        output: Option<PathBuf>,
    },
    Devices {
        #[arg(long)]
        json: bool,
    },
    Inspect {
        image: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Checksum {
        image: PathBuf,
        #[arg(long, default_value = "sha256")]
        algorithm: ChecksumAlgorithm,
        #[arg(long)]
        json: bool,
    },
    Backup {
        target: String,
        output: PathBuf,
    },
    Plan {
        image: PathBuf,
        target: String,
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        windows: WindowsArgs,
        #[arg(long, default_value = "off", value_name = "off|1|2|4")]
        bad_block_check: BadBlockCheck,
    },
    Write {
        image: PathBuf,
        target: String,
        #[arg(long, value_name = "EXACT_PHRASE")]
        confirm: Option<String>,
        /// Emit newline-delimited JSON progress events for trusted clients.
        #[arg(long)]
        json_progress: bool,
        #[command(flatten)]
        windows: WindowsArgs,
        #[arg(long, default_value = "off", value_name = "off|1|2|4")]
        bad_block_check: BadBlockCheck,
    },
    /// One step: fetch (if a catalog slug), plan, then write and verify.
    ///
    /// The target must be named explicitly and must pass the same eligibility
    /// checks as `plan` and `write`. Nothing is written unless --confirm
    /// repeats the exact phrase printed by the plan.
    Flash {
        /// Local image path, or a catalog slug (see `bootable catalog`).
        #[arg(value_name = "SLUG_OR_IMAGE")]
        source: String,
        /// Removable device id or path (see `bootable devices`).
        target: String,
        /// Release index to download when SOURCE is a catalog slug.
        #[arg(long, default_value_t = 0)]
        index: usize,
        /// Where to save a downloaded catalog image.
        #[arg(long, value_name = "ISO_FILE")]
        output: Option<PathBuf>,
        /// Refuse a catalog download unless the publisher's checksum manifest
        /// carries a verified signature from a pinned key (exit 4 otherwise).
        /// Only valid with a catalog slug; a local image has no publisher
        /// signature to check, so combining the two is a usage error.
        #[arg(long)]
        require_signature: bool,
        #[arg(long, value_name = "EXACT_PHRASE")]
        confirm: Option<String>,
        /// Emit newline-delimited JSON progress events for trusted clients.
        #[arg(long)]
        json_progress: bool,
        #[command(flatten)]
        windows: WindowsArgs,
        #[arg(long, default_value = "off", value_name = "off|1|2|4")]
        bad_block_check: BadBlockCheck,
    },
    /// Print a shell completion script to stdout.
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Args)]
struct WindowsArgs {
    #[arg(long, default_value = "gpt", value_name = "gpt|mbr")]
    windows_partition_scheme: bootable_core::WindowsPartitionScheme,
    /// Experimental: also boot Windows installer media on legacy BIOS (CSM)
    /// machines. Needs --windows-partition-scheme mbr; core refuses
    /// unsupported combinations.
    #[arg(long, default_value = "uefi", value_name = "uefi|bios-uefi")]
    windows_boot_firmware: bootable_core::WindowsBootFirmware,
    #[arg(long)]
    bypass_windows_11_requirements: bool,
    #[arg(long)]
    allow_windows_offline_account: bool,
    #[arg(long, value_name = "USERNAME")]
    windows_local_account: Option<String>,
    #[arg(long)]
    copy_windows_regional_options: bool,
    #[arg(long)]
    minimize_windows_data_collection: bool,
    #[arg(long)]
    disable_windows_bitlocker: bool,
    #[arg(long)]
    windows_quality_of_life: bool,
    #[arg(long)]
    use_windows_ca_2023: bool,
    #[arg(long)]
    apply_windows_skusi_policy: bool,
    #[arg(long)]
    force_windows_s_mode: bool,
}

/// Stable process exit codes. Documented in `docs/cli.md`; do not renumber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitStatus {
    Ok = 0,
    Error = 1,
    Usage = 2,
    Confirmation = 3,
    Verification = 4,
}

impl ExitStatus {
    fn code(self) -> u8 {
        self as u8
    }

    fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Usage => "usage",
            Self::Confirmation => "confirmation_required",
            Self::Verification => "verification_failed",
        }
    }
}

/// CLI-level failures that carry their own exit status.
#[derive(Debug)]
enum CliError {
    Usage(String),
    ConfirmationRequired { phrase: String },
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => formatter.write_str(message),
            Self::ConfirmationRequired { phrase } => write!(
                formatter,
                "nothing was written; repeat with --confirm '{phrase}'"
            ),
        }
    }
}

impl std::error::Error for CliError {}

fn core_exit_status(error: &bootable_core::Error) -> ExitStatus {
    use bootable_core::Error;
    match error {
        Error::ConfirmationMismatch { .. } | Error::UnsafeTarget(_) => ExitStatus::Confirmation,
        Error::StalePlan(message)
        | Error::PrivilegedWriteFailed(message)
        | Error::InvalidDownload(message)
            if is_verification_message(message) =>
        {
            ExitStatus::Verification
        }
        _ => ExitStatus::Error,
    }
}

/// Core reports verification failures as message text on a few variants.
fn is_verification_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("verification failed")
        || message.contains("mismatch")
        || message.contains("signature is required")
}

fn exit_status(error: &anyhow::Error) -> ExitStatus {
    if let Some(error) = error.downcast_ref::<CliError>() {
        return match error {
            CliError::Usage(_) => ExitStatus::Usage,
            CliError::ConfirmationRequired { .. } => ExitStatus::Confirmation,
        };
    }
    error
        .downcast_ref::<bootable_core::Error>()
        .map_or(ExitStatus::Error, core_exit_status)
}

fn error_json(message: &str, status: ExitStatus) -> serde_json::Value {
    serde_json::json!({
        "error": { "kind": status.name(), "exit_code": status.code(), "message": message }
    })
}

/// Commands whose success output is JSON also report failures as JSON on stderr.
fn wants_json_errors(command: &Commands) -> bool {
    match command {
        Commands::Catalog { json, .. }
        | Commands::Releases { json, .. }
        | Commands::PiImages { json, .. }
        | Commands::Devices { json }
        | Commands::Inspect { json, .. }
        | Commands::Checksum { json, .. }
        | Commands::Plan { json, .. } => *json,
        _ => false,
    }
}

fn main() -> ExitCode {
    // clap exits with status 2 on usage errors and 0 for --help/--version.
    let cli = Cli::parse();
    let json_errors = cli.command.as_ref().is_some_and(wants_json_errors);
    match run(cli) {
        Ok(()) => ExitCode::from(ExitStatus::Ok.code()),
        Err(error) => {
            let status = exit_status(&error);
            let message = error.to_string();
            if json_errors {
                eprintln!("{}", error_json(&message, status));
            } else {
                eprintln!("error: {message}");
            }
            ExitCode::from(status.code())
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let require_signature = matches!(
        &cli.command,
        Some(
            Commands::Download {
                require_signature: true,
                ..
            } | Commands::Flash {
                require_signature: true,
                ..
            }
        )
    );
    let engine = Bootable::native().require_signature(require_signature);
    match cli.command {
        Some(Commands::Catalog { limit, json }) => print_catalog(&engine, limit, json),
        Some(Commands::Releases { slug, json }) => print_releases(&engine, &slug, json),
        Some(Commands::Download {
            slug,
            index,
            output,
            json_progress,
            require_signature: _,
        }) => download_release(&engine, &slug, index, output, json_progress),
        Some(Commands::PiImages {
            device,
            limit,
            json,
        }) => print_pi_images(&engine, device.as_deref(), limit, json),
        Some(Commands::PiDownload { index, output }) => download_pi_image(&engine, index, output),
        Some(Commands::Devices { json }) => print_devices(&engine, json),
        Some(Commands::Inspect { image, json }) => print_image(&engine, image, json),
        Some(Commands::Checksum {
            image,
            algorithm,
            json,
        }) => print_checksum(&engine, image, algorithm, json),
        Some(Commands::Backup { target, output }) => {
            let mut reporter = ProgressReporter::default();
            engine.backup_device(&target, output, |progress| reporter.print(progress))?;
            Ok(())
        }
        Some(Commands::Plan {
            image,
            target,
            json,
            windows,
            bad_block_check,
        }) => print_plan(
            &engine,
            image,
            &target,
            json,
            write_options(windows, bad_block_check),
        ),
        Some(Commands::Write {
            image,
            target,
            confirm,
            json_progress,
            windows,
            bad_block_check,
        }) => write_image(
            &engine,
            image,
            &target,
            confirm,
            json_progress,
            write_options(windows, bad_block_check),
        ),
        Some(Commands::Flash {
            source,
            target,
            index,
            output,
            require_signature,
            confirm,
            json_progress,
            windows,
            bad_block_check,
        }) => flash_image(
            &engine,
            FlashRequest {
                source,
                target,
                index,
                output,
                require_signature,
                confirm,
                json_progress,
                options: write_options(windows, bad_block_check),
            },
        ),
        Some(Commands::Completions { shell }) => {
            print_completions(shell);
            Ok(())
        }
        None if io::stdout().is_terminal() => run_tui(engine, cli.image),
        None => Err(CliError::Usage(
            "interactive mode needs a terminal; use `bootable --help`".into(),
        )
        .into()),
    }
}

fn print_completions(shell: Shell) {
    clap_complete::generate(shell, &mut Cli::command(), "bootable", &mut io::stdout());
}

fn print_catalog(engine: &Bootable, limit: usize, json: bool) -> Result<()> {
    let distributions = engine.popular_distributions(limit.clamp(1, 100))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&distributions)?);
        return Ok(());
    }
    println!("DistroWatch popularity · six-month page-hit ranking");
    println!("Interest indicator only; not usage, quality, or market share.\n");
    for distro in distributions {
        println!(
            "{:>3}. {:<24} {:>6} hits/day  {}",
            distro.rank,
            distro.name,
            distro.hits_per_day,
            distro.based_on.as_deref().unwrap_or("")
        );
        println!("     slug: {}", distro.slug);
    }
    Ok(())
}

fn print_releases(engine: &Bootable, slug: &str, json: bool) -> Result<()> {
    let details = engine.distribution_details(slug)?;
    let releases = resolve_releases(engine, &details)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&releases)?);
        return Ok(());
    }
    println!("{} ISO releases", details.name);
    if let Some(date) = details.release_date {
        println!("DistroWatch release date: {date}");
    }
    println!();
    for (index, release) in releases.iter().enumerate() {
        println!(
            "[{index}] {}  {}",
            release.name,
            release.size.map(format_bytes).unwrap_or_default()
        );
        println!("    {}", release.url);
        if let (Some(algorithm), Some(checksum)) =
            (release.checksum_algorithm, release.checksum.as_deref())
        {
            println!("    {algorithm}: {checksum}");
        } else if let Some(checksum_url) = release.checksum_url.as_deref() {
            println!(
                "    {} manifest: {checksum_url}",
                release
                    .checksum_algorithm
                    .map(|algorithm| algorithm.to_string())
                    .unwrap_or_else(|| "Checksum".into())
            );
        } else {
            println!("    Publisher checksum unavailable");
        }
    }
    Ok(())
}

fn download_release(
    engine: &Bootable,
    slug: &str,
    index: usize,
    output: Option<PathBuf>,
    json_progress: bool,
) -> Result<()> {
    let mut reporter = ProgressReporter::new(json_progress);
    let result = fetch_catalog_image(engine, slug, index, output, &mut |progress| {
        reporter.print(progress)
    });

    match result {
        Ok(FetchedImage { report, integrity }) => {
            if json_progress {
                reporter.integrity(&integrity);
                reporter.finished();
            } else {
                println!("Ready to write: {}", report.path.display());
                println!("Kind: {}", report.kind);
                println!("Size: {}", format_bytes(report.size));
                reporter.integrity(&integrity);
            }
            Ok(())
        }
        Err(error) => {
            reporter.failed(&error.to_string(), exit_status(&error));
            Err(error)
        }
    }
}

/// A downloaded catalog image and how well it was authenticated.
struct FetchedImage {
    report: ImageReport,
    integrity: IntegrityState,
}

/// The machine-readable integrity summary shared by every JSON surface.
fn integrity_json(integrity: &IntegrityState) -> serde_json::Value {
    serde_json::json!({
        "label": integrity.label(),
        "signature_verified": integrity.is_signature_verified(),
        "signature_expected_but_unverified": integrity.signature_expected_but_unverified(),
    })
}

/// Resolve a catalog slug to a release, then download and verify it.
fn fetch_catalog_image(
    engine: &Bootable,
    slug: &str,
    index: usize,
    output: Option<PathBuf>,
    progress: &mut dyn FnMut(Progress),
) -> Result<FetchedImage> {
    let details = engine.distribution_details(slug)?;
    let releases = resolve_releases(engine, &details)?;
    let release = releases.get(index).ok_or_else(|| {
        CliError::Usage(format!(
            "release index {index} is out of range ({} available)",
            releases.len()
        ))
    })?;
    let destination = output.unwrap_or_else(|| PathBuf::from(&release.name));
    let (report, integrity) = engine.download_iso_with_integrity(
        release,
        &destination,
        &OperationControl::new(),
        progress,
    )?;
    Ok(FetchedImage { report, integrity })
}

fn resolve_releases(engine: &Bootable, details: &DistributionDetails) -> Result<Vec<IsoRelease>> {
    let mut releases = Vec::new();
    let mut last_error = None;
    for source in &details.download_pages {
        match engine.iso_releases(source) {
            Ok(found) => {
                for release in found {
                    if !releases
                        .iter()
                        .any(|existing: &IsoRelease| existing.url == release.url)
                    {
                        releases.push(release);
                    }
                }
            }
            Err(error) => last_error = Some(error),
        }
    }
    if releases.is_empty() {
        if let Some(error) = last_error {
            return Err(error.into());
        }
        bail!("{} has no resolvable ISO releases", details.name);
    }
    Ok(releases)
}

fn print_pi_images(
    engine: &Bootable,
    device: Option<&str>,
    limit: usize,
    json: bool,
) -> Result<()> {
    let catalog = engine.raspberry_pi_catalog()?;
    let images = catalog
        .images
        .into_iter()
        .filter(|image| {
            device.is_none_or(|tag| {
                image.devices.is_empty() || image.devices.iter().any(|item| item == tag)
            })
        })
        .take(limit.clamp(1, 500))
        .collect::<Vec<_>>();
    if json {
        println!("{}", serde_json::to_string_pretty(&images)?);
        return Ok(());
    }
    println!("Raspberry Pi Imager catalog · {} image(s)\n", images.len());
    for (index, image) in images.iter().enumerate() {
        println!(
            "[{index}] {}  {} → {}",
            image.name,
            image.download_size.map(format_bytes).unwrap_or_default(),
            image.extracted_size.map(format_bytes).unwrap_or_default()
        );
        if let Some(description) = &image.description {
            println!("    {description}");
        }
        println!("    {}", image.download_url);
    }
    Ok(())
}

fn download_pi_image(engine: &Bootable, index: usize, output: Option<PathBuf>) -> Result<()> {
    let catalog = engine.raspberry_pi_catalog()?;
    let image = catalog
        .images
        .get(index)
        .with_context(|| format!("Pi image index {index} is out of range"))?;
    let destination = output.unwrap_or_else(|| PathBuf::from(&image.suggested_filename));
    let mut reporter = ProgressReporter::default();
    let report =
        engine.download_pi_image(image, &destination, |progress| reporter.print(progress))?;
    println!("Ready to write: {}", report.path.display());
    println!("Kind: {}", report.kind);
    println!("Size: {}", format_bytes(report.size));
    Ok(())
}

fn print_devices(engine: &Bootable, json: bool) -> Result<()> {
    let devices = engine.discover_devices()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&devices)?);
        return Ok(());
    }
    for device in devices {
        let flags = device_flags(&device);
        println!(
            "{}  {:>9}  {:<24}  {}",
            device.path.display(),
            format_bytes(device.capacity),
            device.display_name(),
            flags
        );
        println!("  id: {}", device.id);
    }
    Ok(())
}

fn print_image(engine: &Bootable, path: PathBuf, json: bool) -> Result<()> {
    let image = engine.inspect_image(path)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&image)?);
    } else {
        println!("Image:    {}", image.path.display());
        println!("Kind:     {}", image.kind);
        println!("Size:     {}", format_bytes(image.size));
        for warning in image.warnings {
            println!("Warning:  {warning}");
        }
    }
    Ok(())
}

fn print_checksum(
    engine: &Bootable,
    path: PathBuf,
    algorithm: ChecksumAlgorithm,
    json: bool,
) -> Result<()> {
    let checksum = engine.checksum_image(path, algorithm)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&checksum)?);
    } else {
        println!("{}  {}", checksum.hexadecimal, checksum.path.display());
    }
    Ok(())
}

fn print_plan(
    engine: &Bootable,
    image: PathBuf,
    target: &str,
    json: bool,
    options: WriteOptions,
) -> Result<()> {
    let plan = engine.prepare_with_options(image, target, options)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        render_plan_text(&plan);
    }
    Ok(())
}

/// The engine operations the write commands depend on. `Bootable` is the real
/// implementation; tests substitute a fake so no device is ever touched.
trait WriteBackend {
    fn check_target(&self, target: &str) -> Result<()>;
    fn fetch(
        &self,
        slug: &str,
        index: usize,
        output: Option<PathBuf>,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<FetchedImage>;
    fn prepare(&self, image: PathBuf, target: &str, options: WriteOptions) -> Result<WritePlan>;
    fn write(
        &self,
        plan: &WritePlan,
        confirmation: &str,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<()>;
}

impl WriteBackend for Bootable {
    fn check_target(&self, target: &str) -> Result<()> {
        let device = self
            .discover_devices()?
            .into_iter()
            .find(|device| device.id.as_str() == target || device.path.to_string_lossy() == target)
            .ok_or_else(|| bootable_core::Error::DeviceNotFound(target.into()))?;
        if !device.is_eligible_target() {
            return Err(bootable_core::Error::UnsafeTarget(format!(
                "{}: {}",
                device.path.display(),
                target_eligibility_label(&device)
            ))
            .into());
        }
        Ok(())
    }

    fn fetch(
        &self,
        slug: &str,
        index: usize,
        output: Option<PathBuf>,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<FetchedImage> {
        fetch_catalog_image(self, slug, index, output, progress)
    }

    fn prepare(&self, image: PathBuf, target: &str, options: WriteOptions) -> Result<WritePlan> {
        Ok(self.prepare_with_options(image, target, options)?)
    }

    fn write(
        &self,
        plan: &WritePlan,
        confirmation: &str,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        Ok(self.write_with_privilege(plan, confirmation, progress)?)
    }
}

fn write_image(
    backend: &impl WriteBackend,
    image: PathBuf,
    target: &str,
    confirmation: Option<String>,
    json_progress: bool,
    options: WriteOptions,
) -> Result<()> {
    let plan = backend
        .prepare(image, target, options)
        .map_err(|error| report_failure(json_progress, error))?;
    confirmed_write(backend, &plan, confirmation, json_progress)
}

/// Emit the terminal `failed` event (JSON mode only) and hand the error back,
/// so early failures obey the same one-terminal-event contract as later ones.
fn report_failure(json_progress: bool, error: anyhow::Error) -> anyhow::Error {
    ProgressReporter::new(json_progress).failed(&error.to_string(), exit_status(&error));
    error
}

/// The single confirmation gate shared by `write` and `flash`: nothing is
/// written unless the caller repeats the plan's exact phrase.
fn confirmed_write(
    backend: &impl WriteBackend,
    plan: &WritePlan,
    confirmation: Option<String>,
    json_progress: bool,
) -> Result<()> {
    let mut reporter = ProgressReporter::new(json_progress);
    let Some(confirmation) = confirmation else {
        if json_progress {
            emit_line(
                serde_json::json!({
                    "event": "confirmation_required",
                    "data": {
                        "confirmation_phrase": plan.confirmation_phrase,
                        "plan": plan,
                    },
                })
                .to_string(),
            );
        } else {
            render_plan_text(plan);
        }
        return Err(CliError::ConfirmationRequired {
            phrase: plan.confirmation_phrase.clone(),
        }
        .into());
    };
    if !plan.confirmation_matches(&confirmation) {
        let error: anyhow::Error = bootable_core::Error::ConfirmationMismatch {
            expected: plan.confirmation_phrase.clone(),
        }
        .into();
        reporter.failed(&error.to_string(), exit_status(&error));
        return Err(error);
    }
    match backend.write(plan, &confirmation, &mut |progress| {
        reporter.print(progress)
    }) {
        Ok(()) => {
            reporter.finished();
            Ok(())
        }
        Err(error) => {
            reporter.failed(&error.to_string(), exit_status(&error));
            Err(error)
        }
    }
}

struct FlashRequest {
    source: String,
    target: String,
    index: usize,
    output: Option<PathBuf>,
    require_signature: bool,
    confirm: Option<String>,
    json_progress: bool,
    options: WriteOptions,
}

#[derive(Debug, PartialEq, Eq)]
enum FlashSource {
    Image(PathBuf),
    Catalog(String),
}

/// An existing file, or anything that looks like a path, is a local image;
/// a bare word is a catalog slug.
fn classify_source(source: &str, exists: bool) -> FlashSource {
    let path_like = source.contains(['/', '\\', '.']) || source.starts_with('~');
    if exists || path_like {
        FlashSource::Image(PathBuf::from(source))
    } else {
        FlashSource::Catalog(source.to_owned())
    }
}

fn flash_image(backend: &impl WriteBackend, request: FlashRequest) -> Result<()> {
    let FlashRequest {
        source,
        target,
        index,
        output,
        require_signature,
        confirm,
        json_progress,
        options,
    } = request;
    let mut reporter = ProgressReporter::new(json_progress);
    // Every failure before the write gate reports exactly one terminal event.
    let image = match classify_source(&source, Path::new(&source).exists()) {
        FlashSource::Image(path) => {
            if require_signature {
                return Err(report_failure(
                    json_progress,
                    CliError::Usage(
                        "--require-signature only applies to catalog downloads; a local \
                         image has no publisher signature to check"
                            .into(),
                    )
                    .into(),
                ));
            }
            path
        }
        FlashSource::Catalog(slug) => {
            // Refuse a missing or ineligible target before a large download.
            backend
                .check_target(&target)
                .map_err(|error| report_failure(json_progress, error))?;
            let FetchedImage { report, integrity } = backend
                .fetch(&slug, index, output, &mut |progress| {
                    reporter.print(progress)
                })
                .map_err(|error| report_failure(json_progress, error))?;
            reporter.integrity(&integrity);
            if confirm.is_none() && !json_progress {
                eprintln!(
                    "Image kept at {}; pass that path instead of the slug to skip the download.",
                    report.path.display()
                );
            }
            report.path
        }
    };
    let plan = backend
        .prepare(image, &target, options)
        .map_err(|error| report_failure(json_progress, error))?;
    confirmed_write(backend, &plan, confirm, json_progress)?;
    if !json_progress {
        println!(
            "Done: {} written and verified on {}",
            plan.image.path.display(),
            plan.target.path.display()
        );
    }
    Ok(())
}

fn write_options(windows: WindowsArgs, bad_block_check: BadBlockCheck) -> WriteOptions {
    WriteOptions {
        windows_partition_scheme: windows.windows_partition_scheme,
        windows_boot_firmware: windows.windows_boot_firmware,
        windows: bootable_core::WindowsExperienceOptions {
            bypass_hardware_requirements: windows.bypass_windows_11_requirements,
            allow_offline_account: windows.allow_windows_offline_account,
            local_account: windows.windows_local_account,
            regional: windows
                .copy_windows_regional_options
                .then(bootable_core::host_regional_options),
            minimize_data_collection: windows.minimize_windows_data_collection,
            disable_bitlocker: windows.disable_windows_bitlocker,
            quality_of_life: windows.windows_quality_of_life,
            use_windows_ca_2023: windows.use_windows_ca_2023,
            apply_skusi_policy: windows.apply_windows_skusi_policy,
            force_s_mode: windows.force_windows_s_mode,
        },
        bad_block_check,
    }
}

#[derive(Default)]
struct ProgressReporter {
    phase: Option<ProgressPhase>,
    percentage: Option<u64>,
    json: bool,
}

impl ProgressReporter {
    fn new(json: bool) -> Self {
        Self {
            phase: None,
            percentage: None,
            json,
        }
    }

    fn print(&mut self, progress: Progress) {
        let percentage = progress
            .total
            .filter(|total| *total > 0)
            .map(|total| progress.completed.saturating_mul(100) / total);
        let phase_changed = self.phase.as_ref() != Some(&progress.phase);
        let percentage_changed = percentage != self.percentage;
        if !phase_changed && !percentage_changed {
            return;
        }
        if self.json {
            emit_line(progress_event_json(&progress));
            let _ = io::Write::flush(&mut io::stdout());
            self.phase = Some(progress.phase);
            self.percentage = percentage;
            return;
        }
        let amount = percentage
            .map(|value| format!("{value:>3}%"))
            .unwrap_or_else(|| "...".into());
        eprintln!("{amount} {:?}: {}", progress.phase, progress.message);
        self.phase = Some(progress.phase);
        self.percentage = percentage;
    }

    fn finished(&self) {
        if self.json {
            emit_line("{\"event\":\"finished\"}".into());
        }
    }

    /// One line (or `integrity` event) saying how the image was authenticated.
    fn integrity(&self, integrity: &IntegrityState) {
        if self.json {
            emit_line(
                serde_json::json!({ "event": "integrity", "data": integrity_json(integrity) })
                    .to_string(),
            );
        } else {
            println!("Integrity: {}", integrity.label());
        }
    }

    fn failed(&self, message: &str, status: ExitStatus) {
        if self.json {
            emit_line(
                serde_json::json!({
                    "event": "failed",
                    "data": {
                        "message": message,
                        "kind": status.name(),
                        "exit_code": status.code(),
                    },
                })
                .to_string(),
            );
        }
    }
}

/// Write one newline-delimited JSON event to stdout. Tests record the events
/// per thread instead so they can assert on the exact stream.
#[cfg(not(test))]
fn emit_line(line: String) {
    println!("{line}");
}

#[cfg(test)]
fn emit_line(line: String) {
    CAPTURED_EVENTS.with(|events| events.borrow_mut().push(line));
}

#[cfg(test)]
thread_local! {
    static CAPTURED_EVENTS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn progress_event_json(progress: &Progress) -> String {
    serde_json::json!({ "event": "progress", "data": progress }).to_string()
}

fn render_plan_text(plan: &WritePlan) {
    println!("Source:   {}", plan.image.path.display());
    println!(
        "Target:   {} ({})",
        plan.target.path.display(),
        plan.target.display_name()
    );
    println!("Strategy: {}", plan.strategy);
    for (index, step) in plan.steps.iter().enumerate() {
        let marker = if step.destructive {
            "ERASES DATA"
        } else {
            "safe"
        };
        println!("  {}. {} [{}]", index + 1, step.title, marker);
    }
    println!("Confirmation: {}", plan.confirmation_phrase);
}

type ImageInspection = std::result::Result<(ImageReport, PathBuf), (String, PathBuf)>;

struct App {
    engine: Bootable,
    devices: Vec<Device>,
    image: Option<ImageReport>,
    image_loading: bool,
    image_receiver: Option<Receiver<ImageInspection>>,
    initial_image: Option<PathBuf>,
    selected: Option<usize>,
    status: String,
    options: WriteOptions,
    advanced: bool,
    checksum_algorithm: ChecksumAlgorithm,
    browse_directory: Option<PathBuf>,
    catalog_open: bool,
    discovery_session: DiscoverySession,
    distributions: Vec<DistributionSummary>,
    popular_distributions: Vec<DistributionSummary>,
    distribution_directory: Vec<DistributionSummary>,
    arch_distributions: Vec<DistributionSummary>,
    debian_distributions: Vec<DistributionSummary>,
    catalog_selected: usize,
    selected_details: Option<DistributionDetails>,
    catalog_releases: Vec<IsoRelease>,
    release_selected: usize,
    pi_catalog: Option<PiCatalog>,
    pi_device_selected: usize,
    pi_image_selected: usize,
    catalog_query: String,
    catalog_searching: bool,
    catalog_visible: usize,
    pi_visible: usize,
    download_session: ManagedDownloadSession,
    downloads_open: bool,
    download_selected: usize,
    download_receiver: Option<Receiver<DownloadUpdate>>,
    catalog_sender: mpsc::Sender<CatalogUpdate>,
    catalog_receiver: Receiver<CatalogUpdate>,
    catalog_focus: CatalogFocus,
    artwork_picker: Picker,
    artwork_key: Option<String>,
    artwork_protocol: Option<StatefulProtocol>,
    artwork_error: Option<String>,
    write_session: ReviewedWriteSession,
    write_receiver: Option<Receiver<WriteUpdate>>,
    hit_regions: HitRegions,
    workspace_focus: WorkspaceFocus,
    preferences: Preferences,
    locale: Locale,
    help_open: bool,
    /// True once core's final (`Finished`) download progress message has been
    /// shown as the status. Replaces comparing the status text, which breaks
    /// as soon as any part of it is localized.
    download_final_message: bool,
}

enum DownloadUpdate {
    Progress(Progress),
    Finished(DownloadCompletion),
}

enum WriteUpdate {
    Progress(Progress),
    Finished(WriteCompletion),
}

enum CatalogUpdate {
    Popular(Result<CatalogFetch<Vec<DistributionSummary>>, String>),
    Directory(Result<CatalogFetch<Vec<DistributionSummary>>, String>),
    RaspberryPi(Result<CatalogFetch<PiCatalog>, String>),
    QuickBase {
        preset: QuickAccess,
        base: &'static str,
        result: Result<CatalogFetch<Vec<DistributionSummary>>, String>,
    },
    Distribution {
        slug: String,
        result: Box<Result<CatalogFetch<DistributionBundle>, String>>,
    },
    Artwork {
        key: String,
        result: Result<Vec<u8>, String>,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum CatalogFocus {
    #[default]
    Distributions,
    Releases,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum WorkspaceFocus {
    #[default]
    Source,
    Target,
    Setup,
    Review,
    Discover,
    Refresh,
}

impl WorkspaceFocus {
    fn next(self, image_available: bool) -> Self {
        match self {
            Self::Source => Self::Target,
            Self::Target if image_available => Self::Setup,
            Self::Target | Self::Setup => Self::Review,
            Self::Review => Self::Discover,
            Self::Discover => Self::Refresh,
            Self::Refresh => Self::Source,
        }
    }

    fn previous(self, image_available: bool) -> Self {
        match self {
            Self::Source => Self::Refresh,
            Self::Target => Self::Source,
            Self::Setup => Self::Target,
            Self::Review if image_available => Self::Setup,
            Self::Review => Self::Target,
            Self::Discover => Self::Review,
            Self::Refresh => Self::Discover,
        }
    }
}

#[derive(Default)]
struct HitRegions {
    open_image: Option<Rect>,
    guide: Option<Rect>,
    language: Option<Rect>,
    recent_rows: Vec<(Rect, usize)>,
    discover: Option<Rect>,
    choose_folder: Option<Rect>,
    windows_options: Option<Rect>,
    windows_offline: Option<Rect>,
    windows_privacy: Option<Rect>,
    windows_bitlocker: Option<Rect>,
    windows_named_account: Option<Rect>,
    windows_regional: Option<Rect>,
    windows_qol: Option<Rect>,
    windows_ca_2023: Option<Rect>,
    windows_skusi_policy: Option<Rect>,
    windows_s_mode: Option<Rect>,
    windows_partition_scheme: Option<Rect>,
    windows_boot_firmware: Option<Rect>,
    advanced: Option<Rect>,
    checksum_algorithm: Option<Rect>,
    bad_blocks: Option<Rect>,
    refresh: Option<Rect>,
    preview: Option<Rect>,
    checksum: Option<Rect>,
    backup: Option<Rect>,
    quit: Option<Rect>,
    device_rows: Vec<(Rect, usize)>,
    catalog_close: Option<Rect>,
    catalog_retry: Option<Rect>,
    catalog_download: Option<Rect>,
    download_pause: Option<Rect>,
    download_cancel: Option<Rect>,
    downloads: Option<Rect>,
    download_rows: Vec<(Rect, usize)>,
    download_retry: Option<Rect>,
    download_use: Option<Rect>,
    download_remove: Option<Rect>,
    source_distrowatch: Option<Rect>,
    source_arch: Option<Rect>,
    source_debian: Option<Rect>,
    source_omarchy: Option<Rect>,
    source_windows: Option<Rect>,
    source_raspberry_pi: Option<Rect>,
    catalog_search: Option<Rect>,
    review_back: Option<Rect>,
    review_write: Option<Rect>,
    confirm_acknowledge: Option<Rect>,
    confirm_cancel: Option<Rect>,
    confirm_write: Option<Rect>,
    distribution_rows: Vec<(Rect, usize)>,
    release_rows: Vec<(Rect, usize)>,
    pi_device_rows: Vec<(Rect, usize)>,
    pi_image_rows: Vec<(Rect, usize)>,
}

impl App {
    /// The active locale bound to the shared catalog. `Copy`, so it can be held
    /// across `&mut self` calls.
    fn t(&self) -> Strings {
        self.locale.strings()
    }

    fn load(engine: Bootable, image_path: Option<PathBuf>, artwork_picker: Picker) -> Self {
        let preferences = Preferences::load();
        let locale = preferences.locale();
        let devices_result = engine.discover_devices();
        let (devices, status) = match devices_result {
            Ok(devices) => {
                let status = locale.strings().format(
                    Message::StatusStartup,
                    &[("media", &removable_media_status_in(locale, &devices))],
                );
                (devices, status)
            }
            Err(error) => (Vec::new(), error.to_string()),
        };
        let initial_image = image_path;
        let image = None;
        let (catalog_sender, catalog_receiver) = mpsc::channel();
        Self {
            engine,
            devices,
            image,
            image_loading: false,
            image_receiver: None,
            initial_image,
            selected: None,
            status,
            options: WriteOptions::default(),
            advanced: false,
            checksum_algorithm: preferences.checksum_algorithm,
            browse_directory: preferences.image_directory(),
            catalog_open: false,
            discovery_session: DiscoverySession::default(),
            distributions: Vec::new(),
            popular_distributions: Vec::new(),
            distribution_directory: Vec::new(),
            arch_distributions: Vec::new(),
            debian_distributions: Vec::new(),
            catalog_selected: 0,
            selected_details: None,
            catalog_releases: Vec::new(),
            release_selected: 0,
            pi_catalog: None,
            pi_device_selected: 0,
            pi_image_selected: 0,
            catalog_query: String::new(),
            catalog_searching: false,
            catalog_visible: 20,
            pi_visible: 20,
            download_session: ManagedDownloadSession::default(),
            downloads_open: false,
            download_selected: 0,
            download_receiver: None,
            catalog_sender,
            catalog_receiver,
            catalog_focus: CatalogFocus::Distributions,
            artwork_picker,
            artwork_key: None,
            artwork_protocol: None,
            artwork_error: None,
            write_session: ReviewedWriteSession::default(),
            write_receiver: None,
            hit_regions: HitRegions::default(),
            workspace_focus: WorkspaceFocus::Source,
            preferences,
            locale,
            help_open: false,
            download_final_message: false,
        }
    }

    fn save_preferences(&mut self) {
        if let Err(error) = self.preferences.save() {
            self.status = self
                .t()
                .format(Message::StatusPrefsSaveFailed, &[("error", &error)]);
        }
    }

    fn remember_image(&mut self, image: &ImageReport) {
        self.preferences.remember_image(image);
        self.save_preferences();
        self.browse_directory = self.preferences.image_directory();
    }

    fn use_recent_image(&mut self, index: usize) {
        match self.preferences.recent_images().get(index) {
            Some(recent) => self.inspect_image_path(recent.path.clone()),
            None => self.status = self.t().text(Message::StatusImageNoRecent).into(),
        }
    }

    fn toggle_help(&mut self) {
        self.help_open = !self.help_open;
    }

    /// Steps System default -> each available language -> System default,
    /// persists the choice, and re-renders in the new language.
    fn cycle_language(&mut self) {
        self.preferences.language = next_language(self.preferences.language);
        self.locale = self.preferences.locale();
        // Cached status text was produced in the previous language; replace it
        // with the current guidance, exactly as the desktop app does.
        self.status = self.review_readiness().guidance_in(self.locale).into();
        self.save_preferences();
    }

    fn load_distrowatch(&mut self) {
        self.load_distrowatch_with(CacheMode::PreferCache);
    }

    fn load_distrowatch_with(&mut self, mode: CacheMode) {
        self.discovery_session.show_distrowatch(QuickAccess::All);
        self.catalog_focus = CatalogFocus::Distributions;
        if !self.popular_distributions.is_empty() && mode == CacheMode::PreferCache {
            self.distributions = self.popular_distributions.clone();
            self.status = self.t().text(Message::StatusCatalogPopularity).into();
        } else if self.discovery_session.begin(CatalogFacet::Popular) {
            self.status = self.t().text(Message::StatusCatalogLoading).into();
            let sender = self.catalog_sender.clone();
            std::thread::spawn(move || {
                let result = Bootable::native()
                    .popular_distributions_cached(100, mode)
                    .map_err(|error| error.to_string());
                let _ = sender.send(CatalogUpdate::Popular(result));
            });
        }
        if (self.distribution_directory.is_empty() || mode == CacheMode::Refresh)
            && !self
                .discovery_session
                .state(CatalogFacet::Directory)
                .is_loading()
        {
            self.load_directory(mode);
        }
    }

    fn load_directory(&mut self, mode: CacheMode) {
        if !self.discovery_session.begin(CatalogFacet::Directory) {
            return;
        }
        let sender = self.catalog_sender.clone();
        std::thread::spawn(move || {
            let result = Bootable::native()
                .distribution_directory_cached(mode)
                .map_err(|error| error.to_string());
            let _ = sender.send(CatalogUpdate::Directory(result));
        });
    }

    fn load_raspberry_pi(&mut self) {
        self.load_raspberry_pi_with(CacheMode::PreferCache);
    }

    fn load_raspberry_pi_with(&mut self, mode: CacheMode) {
        self.discovery_session.show_raspberry_pi();
        self.catalog_focus = CatalogFocus::Distributions;
        if self.pi_catalog.is_some() && mode == CacheMode::PreferCache {
            self.status = self.t().text(Message::StatusCatalogPiSelected).into();
            return;
        }
        if !self.discovery_session.begin(CatalogFacet::RaspberryPi) {
            return;
        }
        self.status = self.t().text(Message::StatusCatalogPiLoading).into();
        let sender = self.catalog_sender.clone();
        std::thread::spawn(move || {
            let result = Bootable::native()
                .raspberry_pi_catalog_cached(mode)
                .map_err(|error| error.to_string());
            let _ = sender.send(CatalogUpdate::RaspberryPi(result));
        });
    }

    fn show_quick_access(&mut self, preset: QuickAccess) {
        self.discovery_session.show_distrowatch(preset);
        self.catalog_query.clear();
        self.catalog_searching = false;
        self.catalog_visible = 20;
        self.selected_details = None;
        self.catalog_releases.clear();
        self.release_selected = 0;
        self.discovery_session.clear_details();
        match preset {
            QuickAccess::All => {
                self.distributions = self.popular_distributions.clone();
                self.status = self.t().text(Message::StatusCatalogPopularity).into();
            }
            QuickAccess::Arch | QuickAccess::Debian => {
                let cached = if preset == QuickAccess::Arch {
                    &self.arch_distributions
                } else {
                    &self.debian_distributions
                };
                if cached.is_empty() {
                    self.distributions.clear();
                    self.load_quick_base(preset, CacheMode::PreferCache);
                } else {
                    self.distributions = cached.clone();
                }
            }
            QuickAccess::Omarchy => {
                if let Some(omarchy) = self
                    .distribution_directory
                    .iter()
                    .chain(self.popular_distributions.iter())
                    .find(|distribution| distribution.slug == "omarchy")
                    .cloned()
                {
                    self.distributions = vec![omarchy];
                    self.status = self.t().text(Message::StatusCatalogOmarchy).into();
                } else {
                    self.status = self.t().text(Message::StatusCatalogOmarchyMissing).into();
                }
            }
            QuickAccess::Windows => {
                self.distributions.clear();
                self.status = self.t().text(Message::StatusCatalogWindowsTools).into();
            }
        }
        self.catalog_selected = 0;
    }

    fn load_quick_base(&mut self, preset: QuickAccess, mode: CacheMode) {
        let base = if preset == QuickAccess::Arch {
            "Arch"
        } else {
            "Debian"
        };
        let facet = if preset == QuickAccess::Arch {
            CatalogFacet::Arch
        } else {
            CatalogFacet::Debian
        };
        if !self.discovery_session.begin(facet) {
            return;
        }
        self.status = self
            .t()
            .format(Message::StatusCatalogLoadingBase, &[("base", &base)]);
        let sender = self.catalog_sender.clone();
        std::thread::spawn(move || {
            let result = Bootable::native()
                .distributions_based_on_cached(base, mode)
                .map_err(|error| error.to_string());
            let _ = sender.send(CatalogUpdate::QuickBase {
                preset,
                base,
                result,
            });
        });
    }

    /// The status after an ISO release is picked: whether the publisher's
    /// checksum can be verified or only HTTPS length and boot structure.
    fn iso_selected_status(&self) -> String {
        let has_checksum = self
            .catalog_releases
            .get(self.release_selected)
            .is_some_and(|release| release.checksum.is_some() || release.checksum_url.is_some());
        self.t()
            .text(if has_checksum {
                Message::StatusCatalogIsoSelectedChecksum
            } else {
                Message::StatusCatalogIsoSelectedHttps
            })
            .into()
    }

    fn toggle_catalog(&mut self) {
        if self.catalog_open {
            self.catalog_open = false;
            self.status = self.t().format(
                Message::StatusCatalogClosed,
                &[(
                    "guidance",
                    &self.review_readiness().guidance_in(self.locale),
                )],
            );
            return;
        }
        self.catalog_open = true;
        match self.discovery_session.source() {
            DiscoverySource::DistroWatch => self.load_distrowatch(),
            DiscoverySource::RaspberryPi => self.load_raspberry_pi(),
        }
    }

    fn select_catalog_distribution(&mut self, index: usize) {
        self.select_catalog_distribution_with(index, CacheMode::PreferCache);
    }

    fn select_catalog_distribution_with(&mut self, index: usize, mode: CacheMode) {
        let Some(distribution) = self.distributions.get(index).cloned() else {
            return;
        };
        self.catalog_selected = index;
        self.release_selected = 0;
        self.selected_details = None;
        self.catalog_releases.clear();
        self.discovery_session
            .expect_details(distribution.slug.clone());
        self.status = self.t().format(
            Message::StatusCatalogLoadingReleases,
            &[("name", &distribution.name)],
        );
        let slug = distribution.slug;
        let request_slug = slug.clone();
        let sender = self.catalog_sender.clone();
        std::thread::spawn(move || {
            let result = Bootable::native()
                .distribution_bundle_cached(&slug, mode)
                .map_err(|error| error.to_string());
            let _ = sender.send(CatalogUpdate::Distribution {
                slug: request_slug,
                result: Box::new(result),
            });
        });
    }

    fn retry_catalog(&mut self) {
        if self.discovery_session.source() == DiscoverySource::RaspberryPi {
            self.load_raspberry_pi_with(CacheMode::Refresh);
            return;
        }
        match self.discovery_session.quick_access() {
            QuickAccess::All | QuickAccess::Omarchy => {
                if !matches!(
                    self.discovery_session.state(CatalogFacet::Details),
                    CatalogState::Idle
                ) && self.distributions.get(self.catalog_selected).is_some()
                {
                    self.select_catalog_distribution_with(
                        self.catalog_selected,
                        CacheMode::Refresh,
                    );
                } else {
                    self.load_distrowatch_with(CacheMode::Refresh);
                }
            }
            QuickAccess::Arch | QuickAccess::Debian => {
                self.load_quick_base(self.discovery_session.quick_access(), CacheMode::Refresh);
            }
            QuickAccess::Windows => {
                self.status = self.t().text(Message::StatusCatalogWindowsUsesIso).into();
            }
        }
    }

    fn desired_catalog_artwork(&self) -> Option<String> {
        if !self.catalog_open || self.discovery_session.quick_access() == QuickAccess::Windows {
            return None;
        }
        match self.discovery_session.source() {
            DiscoverySource::DistroWatch => self
                .selected_details
                .as_ref()
                .and_then(|details| {
                    details
                        .screenshot_url
                        .as_ref()
                        .or(details.logo_url.as_ref())
                })
                .cloned()
                .or_else(|| {
                    self.distributions
                        .get(self.catalog_selected)
                        .map(|distribution| distribution.logo_url.clone())
                }),
            DiscoverySource::RaspberryPi => self.pi_catalog.as_ref().and_then(|catalog| {
                catalog
                    .images
                    .get(self.pi_image_selected)
                    .and_then(|image| image.icon_url.clone())
                    .or_else(|| {
                        catalog
                            .devices
                            .get(self.pi_device_selected)
                            .and_then(|device| device.icon_url.clone())
                    })
            }),
        }
    }

    fn sync_catalog_artwork(&mut self) {
        let desired = self.desired_catalog_artwork();
        if desired == self.artwork_key {
            return;
        }
        self.artwork_key = desired.clone();
        self.artwork_protocol = None;
        self.artwork_error = None;
        let Some(key) = desired else {
            return;
        };
        let request_key = key.clone();
        let sender = self.catalog_sender.clone();
        std::thread::spawn(move || {
            let result = Bootable::native()
                .catalog_artwork(&request_key)
                .map_err(|error| error.to_string());
            let _ = sender.send(CatalogUpdate::Artwork { key, result });
        });
    }

    fn poll_catalog(&mut self) {
        let locale = self.locale;
        let t = self.t();
        while let Ok(update) = self.catalog_receiver.try_recv() {
            match update {
                CatalogUpdate::Popular(result) => match result {
                    Ok(fetch) => {
                        let source = fetch.source_label_in(locale);
                        self.discovery_session.complete(
                            CatalogFacet::Popular,
                            &fetch,
                            fetch.value.is_empty(),
                        );
                        let distributions = fetch.value;
                        let count = distributions.len();
                        self.popular_distributions = distributions.clone();
                        if self.discovery_session.source() == DiscoverySource::DistroWatch
                            && self.discovery_session.quick_access() == QuickAccess::All
                            && self.catalog_query.is_empty()
                        {
                            self.distributions = distributions;
                            self.catalog_selected = 0;
                            self.status = t.plural(
                                Message::StatusCatalogDistributionsLoaded,
                                count as u64,
                                &[("source", &source)],
                            );
                            if count > 0 {
                                self.select_catalog_distribution(0);
                            }
                        }
                    }
                    Err(error) => {
                        self.discovery_session
                            .fail(CatalogFacet::Popular, error.clone());
                        self.status = self
                            .discovery_session
                            .state(CatalogFacet::Popular)
                            .short_label_in(locale, t.text(Message::CatalogSubjectDistributions));
                    }
                },
                CatalogUpdate::Directory(result) => match result {
                    Ok(fetch) => {
                        self.discovery_session.complete(
                            CatalogFacet::Directory,
                            &fetch,
                            fetch.value.is_empty(),
                        );
                        let directory = fetch.value;
                        self.distribution_directory = directory.clone();
                        if !self.catalog_query.is_empty() {
                            self.distributions = directory;
                            self.catalog_selected = 0;
                            self.status = catalog_search_summary(
                                &self.catalog_query,
                                self.filtered_distribution_indices().len(),
                            );
                        }
                    }
                    Err(error) => {
                        self.discovery_session
                            .fail(CatalogFacet::Directory, error.clone());
                        if !self.catalog_query.is_empty() {
                            self.status = self
                                .discovery_session
                                .state(CatalogFacet::Directory)
                                .short_label_in(
                                    locale,
                                    t.text(Message::CatalogSubjectSearchCatalog),
                                );
                        }
                    }
                },
                CatalogUpdate::RaspberryPi(result) => match result {
                    Ok(fetch) => {
                        self.discovery_session.complete(
                            CatalogFacet::RaspberryPi,
                            &fetch,
                            fetch.value.images.is_empty(),
                        );
                        let source = fetch.source_label_in(locale);
                        let catalog = fetch.value;
                        let count = catalog.images.len();
                        self.pi_catalog = Some(catalog);
                        self.pi_device_selected = 0;
                        self.pi_image_selected = 0;
                        if self.discovery_session.source() == DiscoverySource::RaspberryPi {
                            self.status = t.plural(
                                Message::StatusCatalogPiImagesLoaded,
                                count as u64,
                                &[("source", &source)],
                            );
                        }
                    }
                    Err(error)
                        if self.discovery_session.source() == DiscoverySource::RaspberryPi =>
                    {
                        self.discovery_session
                            .fail(CatalogFacet::RaspberryPi, error);
                        self.status = self
                            .discovery_session
                            .state(CatalogFacet::RaspberryPi)
                            .short_label_in(locale, t.text(Message::CatalogSubjectPiImages));
                    }
                    Err(error) => self
                        .discovery_session
                        .fail(CatalogFacet::RaspberryPi, error),
                },
                CatalogUpdate::QuickBase {
                    preset,
                    base,
                    result,
                } => match result {
                    Ok(fetch) => {
                        let facet = if preset == QuickAccess::Arch {
                            CatalogFacet::Arch
                        } else {
                            CatalogFacet::Debian
                        };
                        self.discovery_session
                            .complete(facet, &fetch, fetch.value.is_empty());
                        let source = fetch.source_label_in(locale);
                        let distributions = fetch.value;
                        let count = distributions.len();
                        if preset == QuickAccess::Arch {
                            self.arch_distributions = distributions.clone();
                        } else {
                            self.debian_distributions = distributions.clone();
                        }
                        if self.discovery_session.quick_access() == preset {
                            self.distributions = distributions;
                            self.catalog_selected = 0;
                            self.status = t.plural(
                                Message::StatusCatalogBaseLoaded,
                                count as u64,
                                &[("base", &base), ("source", &source)],
                            );
                        }
                    }
                    Err(error) => {
                        let facet = if preset == QuickAccess::Arch {
                            CatalogFacet::Arch
                        } else {
                            CatalogFacet::Debian
                        };
                        self.discovery_session.fail(facet, error);
                        if self.discovery_session.quick_access() == preset {
                            self.status = self.discovery_session.state(facet).short_label_in(
                                locale,
                                &t.format(
                                    Message::CatalogSubjectBaseDistributions,
                                    &[("base", &base)],
                                ),
                            );
                        }
                    }
                },
                CatalogUpdate::Distribution { slug, result } => {
                    if !self.discovery_session.accepts_details(&slug) {
                        continue;
                    }
                    match *result {
                        Ok(fetch) => {
                            self.discovery_session.complete(
                                CatalogFacet::Details,
                                &fetch,
                                fetch.value.releases.is_empty(),
                            );
                            let source = fetch.source_label_in(locale);
                            let DistributionBundle {
                                details,
                                releases,
                                warnings,
                            } = fetch.value;
                            let count = releases.len();
                            self.selected_details = Some(details);
                            self.catalog_releases = releases;
                            self.release_selected = 0;
                            self.catalog_focus = CatalogFocus::Releases;
                            let releases_summary = t.plural(
                                Message::StatusCatalogReleasesLoaded,
                                count as u64,
                                &[("source", &source)],
                            );
                            self.status = if count == 0 && !warnings.is_empty() {
                                t.plural(
                                    Message::StatusCatalogProfileReadyErrors,
                                    warnings.len() as u64,
                                    &[],
                                )
                            } else if count == 0 {
                                t.text(Message::StatusCatalogProfileReadyNoIso).into()
                            } else if !warnings.is_empty() {
                                t.format(
                                    Message::StatusCatalogWithWarnings,
                                    &[
                                        ("summary", &releases_summary),
                                        (
                                            "warnings",
                                            &t.plural(
                                                Message::StatusCatalogSourceWarnings,
                                                warnings.len() as u64,
                                                &[],
                                            ),
                                        ),
                                    ],
                                )
                            } else {
                                releases_summary
                            };
                        }
                        Err(error) => {
                            self.discovery_session.fail(CatalogFacet::Details, error);
                            self.status = self
                                .discovery_session
                                .state(CatalogFacet::Details)
                                .short_label_in(locale, t.text(Message::CatalogSubjectIsoReleases));
                        }
                    }
                }
                CatalogUpdate::Artwork { key, result } => {
                    if self.artwork_key.as_deref() != Some(key.as_str()) {
                        continue;
                    }
                    match result {
                        Ok(bytes) => match image::load_from_memory(&bytes) {
                            Ok(image) => {
                                self.artwork_protocol =
                                    Some(self.artwork_picker.new_resize_protocol(image));
                                self.artwork_error = None;
                            }
                            Err(error) => {
                                self.artwork_error = Some(t.format(
                                    Message::StatusCatalogArtworkError,
                                    &[("error", &error)],
                                ));
                            }
                        },
                        Err(error) => self.artwork_error = Some(error),
                    }
                }
            }
        }
    }

    fn refresh_download_jobs(&mut self) {
        match self.download_session.refresh(&self.engine) {
            Ok(jobs) => {
                self.download_selected = self.download_selected.min(jobs.len().saturating_sub(1));
            }
            Err(error) => {
                self.status = self.t().format(
                    Message::StatusDownloadHistoryUnavailable,
                    &[("error", &error)],
                );
            }
        }
    }

    fn toggle_downloads(&mut self) {
        self.downloads_open = !self.downloads_open;
        if self.downloads_open {
            self.refresh_download_jobs();
            self.status = self.t().plural(
                Message::DownloadsJobsInHistory,
                self.download_session.jobs().len() as u64,
                &[],
            );
        }
    }

    fn launch_download_job(&mut self, id: String, destination: PathBuf, retry: bool) {
        let DownloadRequest::Launch(launch) = self.download_session.request(id, destination, retry)
        else {
            self.status = self.t().text(Message::StatusDownloadQueued).into();
            self.refresh_download_jobs();
            return;
        };
        self.launch_download_worker(launch);
    }

    fn launch_download_worker(&mut self, launch: DownloadLaunch) {
        self.download_final_message = false;
        self.status = self
            .t()
            .text(if launch.retry {
                Message::StatusDownloadRetrying
            } else {
                Message::StatusDownloadStarting
            })
            .into();
        let DownloadLaunch {
            id,
            destination,
            retry,
            control,
        } = launch;
        let completed_destination = destination;
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let progress_sender = sender.clone();
            let engine = Bootable::native();
            let result = if retry {
                engine.retry_download_job(&id, &control, move |progress| {
                    let _ = progress_sender.send(DownloadUpdate::Progress(progress));
                })
            } else {
                engine.run_download_job(&id, &control, move |progress| {
                    let _ = progress_sender.send(DownloadUpdate::Progress(progress));
                })
            }
            .map(|report| (report, completed_destination));
            let _ = sender.send(DownloadUpdate::Finished(DownloadCompletion::from_result(
                result,
            )));
        });
        self.download_receiver = Some(receiver);
    }

    fn retry_selected_download(&mut self) {
        let Some(job) = self.download_session.jobs().get(self.download_selected) else {
            self.status = self.t().text(Message::StatusDownloadChooseJob).into();
            return;
        };
        let id = job.id.clone();
        match self.download_session.retry(&self.engine, &id) {
            Ok(DownloadRequest::Launch(launch)) => self.launch_download_worker(launch),
            Ok(DownloadRequest::Queued) => {
                self.status = self.t().text(Message::StatusDownloadRetryQueued).into()
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    fn start_next_queued_download(&mut self) {
        match self.download_session.next_queued(&self.engine) {
            Ok(Some(launch)) => self.launch_download_worker(launch),
            Ok(None) => {}
            Err(error) => {
                self.status = self
                    .t()
                    .format(Message::StatusDownloadStartFailed, &[("error", &error)]);
            }
        }
    }

    fn use_selected_download(&mut self) {
        let Some(job) = self.download_session.jobs().get(self.download_selected) else {
            self.status = self.t().text(Message::StatusDownloadChooseJob).into();
            return;
        };
        let id = job.id.clone();
        let destination = job.destination.clone();
        match self.download_session.use_completed(&self.engine, &id) {
            Ok(report) => {
                self.remember_image(&report);
                self.reset_image_scoped_options();
                self.image = Some(report);
                self.advanced = false;
                self.downloads_open = false;
                self.status = self.t().format(
                    Message::StatusDownloadUsingCompleted,
                    &[("path", &destination.display())],
                );
            }
            Err(error) => {
                self.status = self.t().format(
                    Message::StatusDownloadCompletedUnavailable,
                    &[("error", &error)],
                );
            }
        }
    }

    fn remove_selected_download(&mut self) {
        let Some(job) = self.download_session.jobs().get(self.download_selected) else {
            self.status = self.t().text(Message::StatusDownloadChooseJob).into();
            return;
        };
        let id = job.id.clone();
        match self.download_session.remove(&self.engine, &id) {
            Ok(()) => {
                self.status = self.t().text(Message::StatusDownloadHistoryRemoved).into();
                self.refresh_download_jobs();
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    fn handle_download_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::Char('m') => self.toggle_downloads(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.download_selected = self.download_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.download_selected = (self.download_selected + 1)
                    .min(self.download_session.jobs().len().saturating_sub(1));
            }
            KeyCode::Char('r') => self.retry_selected_download(),
            KeyCode::Enter | KeyCode::Char('u') => self.use_selected_download(),
            KeyCode::Delete | KeyCode::Char('x') => self.remove_selected_download(),
            _ => {}
        }
    }

    fn download_catalog_release(&mut self) {
        let Some(release) = self.catalog_releases.get(self.release_selected).cloned() else {
            self.status = self.t().text(Message::StatusCatalogChooseRelease).into();
            return;
        };
        let mut dialog = rfd::FileDialog::new()
            .add_filter(self.t().text(Message::SourceDialogFilterIso), &["iso"])
            .set_file_name(&release.name);
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        let Some(destination) = dialog.save_file() else {
            self.status = self.t().text(Message::StatusDownloadIsoCancelled).into();
            return;
        };
        match self.engine.enqueue_iso_download(&release, &destination) {
            Ok(id) => self.launch_download_job(id, destination, false),
            Err(error) => self.status = error.to_string(),
        }
    }

    fn open_selected_distrowatch_page(&mut self) {
        let Some(page_url) = self
            .distributions
            .get(self.catalog_selected)
            .map(|distribution| distribution.page_url.clone())
        else {
            self.status = self
                .t()
                .text(Message::StatusCatalogChooseDistribution)
                .into();
            return;
        };
        self.status = match self.engine.open_distrowatch_page(&page_url) {
            Ok(()) => self.t().text(Message::StatusCatalogBrowserOpened).into(),
            Err(error) => error.to_string(),
        };
    }

    fn download_pi_catalog_image(&mut self) {
        let Some(image) = self
            .pi_catalog
            .as_ref()
            .and_then(|catalog| catalog.images.get(self.pi_image_selected))
            .cloned()
        else {
            self.status = self.t().text(Message::StatusCatalogPiChoose).into();
            return;
        };
        let mut dialog = rfd::FileDialog::new().set_file_name(&image.suggested_filename);
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        let Some(destination) = dialog.save_file() else {
            self.status = self.t().text(Message::StatusDownloadPiCancelled).into();
            return;
        };
        match self.engine.enqueue_pi_download(&image, &destination) {
            Ok(id) => self.launch_download_job(id, destination, false),
            Err(error) => self.status = error.to_string(),
        }
    }

    /// Shows core's progress message as the status. Core's last message
    /// (phase `Finished`) names the integrity result, for example a verified
    /// signature; remember that it is showing so the generic ready line does
    /// not replace it. This is an explicit flag, not a comparison of the
    /// (localizable) status text.
    fn show_download_progress(&mut self, progress: &Progress) {
        self.download_final_message = progress.phase == ProgressPhase::Finished;
        self.status = progress.message.clone();
    }

    /// The download finished and its image is in use: keep core's final
    /// message when it was shown, otherwise the shared ready line.
    fn show_download_ready(&mut self, path: &Path) {
        if !self.download_final_message {
            self.status = self
                .t()
                .format(Message::StatusDownloadReady, &[("name", &path.display())]);
        }
        self.download_final_message = false;
    }

    fn poll_download(&mut self) {
        let Some(receiver) = self.download_receiver.take() else {
            return;
        };
        let mut finished = false;
        while let Ok(update) = receiver.try_recv() {
            match update {
                DownloadUpdate::Progress(progress) => {
                    self.show_download_progress(&progress);
                    self.download_session.apply_progress(progress);
                }
                DownloadUpdate::Finished(completion) => {
                    finished = true;
                    match &completion {
                        DownloadCompletion::Ready {
                            report,
                            destination,
                        } => {
                            self.browse_directory = destination.parent().map(PathBuf::from);
                            self.show_download_ready(&report.path);
                            self.reset_image_scoped_options();
                            self.image = Some(report.clone());
                            self.advanced = false;
                        }
                        DownloadCompletion::Cancelled => {
                            self.download_final_message = false;
                            self.status = self
                                .t()
                                .text(Message::StatusDownloadCancelledCleaned)
                                .into();
                        }
                        DownloadCompletion::Failed(error) => {
                            self.download_final_message = false;
                            self.status = self
                                .t()
                                .format(Message::StatusDownloadStopped, &[("error", error)]);
                        }
                    }
                    self.download_session.finish(completion);
                    self.refresh_download_jobs();
                    self.start_next_queued_download();
                }
            }
        }
        if !finished {
            self.download_receiver = Some(receiver);
        }
    }

    fn toggle_download_pause(&mut self) {
        match self.download_session.toggle_pause(&self.engine) {
            Ok(Some(OperationState::Paused)) => {
                // The shared line has no key legend; the TUI appends its own.
                self.status = format!("{} (p / x)", self.t().text(Message::StatusDownloadPaused));
            }
            Ok(Some(OperationState::Running)) => {
                self.status = self.t().text(Message::StatusDownloadResumed).into();
            }
            Ok(Some(OperationState::Cancelled) | None) => {}
            Err(error) => self.status = error.to_string(),
        }
    }

    fn cancel_download(&mut self) {
        if self.download_session.cancel() {
            self.status = self.t().text(Message::StatusDownloadCancelling).into();
        }
    }

    fn handle_catalog_key(&mut self, code: KeyCode) {
        if self.discovery_session.quick_access() == QuickAccess::Windows {
            match code {
                KeyCode::Esc | KeyCode::Char('g') => self.toggle_catalog(),
                KeyCode::Char('o') | KeyCode::Enter => self.choose_image(),
                KeyCode::Char('w') => self.toggle_windows_requirements(),
                KeyCode::Char('n') => self.toggle_windows_offline_account(),
                KeyCode::Char('v') => self.toggle_windows_privacy(),
                KeyCode::Char('l') => self.toggle_windows_bitlocker(),
                KeyCode::Char('a') => self.toggle_windows_named_account(),
                KeyCode::Char('r') => self.toggle_windows_regional(),
                KeyCode::Char('y') => self.toggle_windows_qol(),
                KeyCode::Char('c') => self.toggle_windows_ca_2023(),
                KeyCode::Char('k') => self.toggle_windows_skusi_policy(),
                KeyCode::Char('s') => self.toggle_windows_s_mode(),
                KeyCode::Char('p') => self.cycle_windows_partition_scheme(),
                KeyCode::Char('f') => self.cycle_windows_boot_firmware(),
                KeyCode::Char('1') => self.show_quick_access(QuickAccess::All),
                KeyCode::Char('2') => self.show_quick_access(QuickAccess::Arch),
                KeyCode::Char('3') => self.show_quick_access(QuickAccess::Debian),
                KeyCode::Char('4') => self.show_quick_access(QuickAccess::Omarchy),
                KeyCode::Char('6') => self.load_raspberry_pi(),
                _ => {}
            }
            return;
        }
        if self.catalog_searching {
            match code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.catalog_searching = false;
                    self.status = if self.catalog_query.is_empty() {
                        self.t().text(Message::StatusCatalogSearchClosed).into()
                    } else {
                        catalog_search_summary(
                            &self.catalog_query,
                            self.filtered_distribution_indices().len(),
                        )
                    };
                }
                KeyCode::Backspace => {
                    self.catalog_query.pop();
                    self.reset_catalog_search();
                }
                KeyCode::Char(character) => {
                    self.catalog_query.push(character);
                    self.reset_catalog_search();
                }
                _ => {}
            }
            return;
        }
        if code == KeyCode::Char('o') {
            self.choose_image();
            return;
        }
        if code == KeyCode::Char('/') {
            self.catalog_searching = true;
            self.status = format!(
                "{} · Esc",
                self.t().text(Message::DiscoverSearchPlaceholder)
            );
            return;
        }
        if code == KeyCode::Char('r') {
            self.retry_catalog();
            return;
        }
        if code == KeyCode::Char('b')
            && self.discovery_session.source() == DiscoverySource::DistroWatch
            && self.catalog_releases.is_empty()
        {
            self.open_selected_distrowatch_page();
            return;
        }
        if code == KeyCode::Char('1') {
            self.show_quick_access(QuickAccess::All);
            return;
        }
        if code == KeyCode::Char('2') {
            self.show_quick_access(QuickAccess::Arch);
            return;
        }
        if code == KeyCode::Char('3') {
            self.show_quick_access(QuickAccess::Debian);
            return;
        }
        if code == KeyCode::Char('4') {
            self.show_quick_access(QuickAccess::Omarchy);
            return;
        }
        if code == KeyCode::Char('5') {
            self.show_quick_access(QuickAccess::Windows);
            return;
        }
        if code == KeyCode::Char('6') {
            self.load_raspberry_pi();
            return;
        }
        if self.discovery_session.source() == DiscoverySource::RaspberryPi {
            self.handle_pi_catalog_key(code);
            return;
        }
        match code {
            KeyCode::Esc | KeyCode::Char('g') => self.toggle_catalog(),
            KeyCode::Left | KeyCode::BackTab => {
                self.catalog_focus = CatalogFocus::Distributions;
            }
            KeyCode::Right | KeyCode::Tab => {
                if !self.catalog_releases.is_empty() {
                    self.catalog_focus = CatalogFocus::Releases;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => match self.catalog_focus {
                CatalogFocus::Distributions => self.move_distribution(-1),
                CatalogFocus::Releases => {
                    self.release_selected = self.release_selected.saturating_sub(1);
                }
            },
            KeyCode::Down | KeyCode::Char('j') => match self.catalog_focus {
                CatalogFocus::Distributions => self.move_distribution(1),
                CatalogFocus::Releases => {
                    self.release_selected = (self.release_selected + 1)
                        .min(self.catalog_releases.len().saturating_sub(1));
                }
            },
            KeyCode::Enter => match self.catalog_focus {
                CatalogFocus::Distributions => {
                    self.select_catalog_distribution(self.catalog_selected);
                }
                CatalogFocus::Releases => self.download_catalog_release(),
            },
            KeyCode::Char('d') if !self.catalog_releases.is_empty() => {
                self.download_catalog_release();
            }
            _ => {}
        }
    }

    fn handle_pi_catalog_key(&mut self, code: KeyCode) {
        let device_count = self
            .pi_catalog
            .as_ref()
            .map(|catalog| catalog.devices.len())
            .unwrap_or_default();
        match code {
            KeyCode::Esc | KeyCode::Char('g') => self.toggle_catalog(),
            KeyCode::Left | KeyCode::BackTab => {
                self.catalog_focus = CatalogFocus::Distributions;
            }
            KeyCode::Right | KeyCode::Tab => {
                self.catalog_focus = CatalogFocus::Releases;
            }
            KeyCode::Up | KeyCode::Char('k') => match self.catalog_focus {
                CatalogFocus::Distributions => {
                    self.pi_device_selected = self.pi_device_selected.saturating_sub(1);
                }
                CatalogFocus::Releases => self.move_pi_image(-1),
            },
            KeyCode::Down | KeyCode::Char('j') => match self.catalog_focus {
                CatalogFocus::Distributions => {
                    self.pi_device_selected =
                        (self.pi_device_selected + 1).min(device_count.saturating_sub(1));
                }
                CatalogFocus::Releases => self.move_pi_image(1),
            },
            KeyCode::Enter if self.catalog_focus == CatalogFocus::Distributions => {
                self.select_first_pi_image_for_device();
                self.catalog_focus = CatalogFocus::Releases;
            }
            KeyCode::Enter | KeyCode::Char('d') => self.download_pi_catalog_image(),
            _ => {}
        }
    }

    fn select_first_pi_image_for_device(&mut self) {
        if let Some(index) = self.compatible_pi_image_indices().first().copied() {
            self.pi_image_selected = index;
            if let Some(device) = self
                .pi_catalog
                .as_ref()
                .and_then(|catalog| catalog.devices.get(self.pi_device_selected))
            {
                self.status = self.t().format(
                    Message::StatusCatalogPiCompatible,
                    &[("board", &device.name)],
                );
            }
        }
    }

    fn compatible_pi_image_indices(&self) -> Vec<usize> {
        let Some(catalog) = &self.pi_catalog else {
            return Vec::new();
        };
        let tags = catalog
            .devices
            .get(self.pi_device_selected)
            .map(|device| device.tags.as_slice())
            .unwrap_or_default();
        catalog
            .images
            .iter()
            .enumerate()
            .filter(|(_, image)| {
                let device_matches = tags.is_empty()
                    || image.devices.is_empty()
                    || image
                        .devices
                        .iter()
                        .any(|tag| tags.iter().any(|selected| selected == tag));
                let query = self.catalog_query.to_lowercase();
                let search_matches = query.is_empty()
                    || image.name.to_lowercase().contains(&query)
                    || image
                        .description
                        .as_deref()
                        .is_some_and(|value| value.to_lowercase().contains(&query))
                    || image
                        .category
                        .as_deref()
                        .is_some_and(|value| value.to_lowercase().contains(&query));
                device_matches && search_matches
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn move_pi_image(&mut self, direction: i8) {
        let indices = self.compatible_pi_image_indices();
        let position = indices
            .iter()
            .position(|index| *index == self.pi_image_selected)
            .unwrap_or_default();
        let next = if direction < 0 {
            position.saturating_sub(1)
        } else {
            (position + 1).min(indices.len().saturating_sub(1))
        };
        if let Some(index) = indices.get(next) {
            self.pi_image_selected = *index;
        }
        if direction > 0 && next + 2 >= self.pi_visible && self.pi_visible < indices.len() {
            self.pi_visible = self.pi_visible.saturating_add(20);
        }
    }

    fn filtered_distribution_indices(&self) -> Vec<usize> {
        self.distributions
            .iter()
            .enumerate()
            .filter(|(_, distribution)| {
                distribution_matches_query(distribution, &self.catalog_query)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn move_distribution(&mut self, direction: i8) {
        let indices = self.filtered_distribution_indices();
        let position = indices
            .iter()
            .position(|index| *index == self.catalog_selected)
            .unwrap_or_default();
        let next = if direction < 0 {
            position.saturating_sub(1)
        } else {
            (position + 1).min(indices.len().saturating_sub(1))
        };
        if let Some(index) = indices.get(next) {
            self.catalog_selected = *index;
        }
        if direction > 0 && next + 2 >= self.catalog_visible && self.catalog_visible < indices.len()
        {
            self.catalog_visible = self.catalog_visible.saturating_add(20);
        }
    }

    fn reset_catalog_search(&mut self) {
        self.catalog_visible = 20;
        self.pi_visible = 20;
        if !self.catalog_query.is_empty() {
            self.discovery_session.show_distrowatch(QuickAccess::All);
        }
        self.selected_details = None;
        self.catalog_releases.clear();
        self.release_selected = 0;
        self.discovery_session.clear_details();
        self.distributions = if self.catalog_query.is_empty() {
            self.popular_distributions.clone()
        } else {
            if self.distribution_directory.is_empty() {
                self.load_directory(CacheMode::PreferCache);
            }
            self.distribution_directory.clone()
        };
        if !self.catalog_query.is_empty() && !self.distribution_directory.is_empty() {
            self.status = catalog_search_summary(
                &self.catalog_query,
                self.filtered_distribution_indices().len(),
            );
        }
        if let Some(index) = self.filtered_distribution_indices().first() {
            self.catalog_selected = *index;
        }
        if let Some(index) = self.compatible_pi_image_indices().first() {
            self.pi_image_selected = *index;
        }
    }

    fn choose_image(&mut self) {
        if self.image_loading {
            self.status = self.t().text(Message::StatusImageBusy).into();
            return;
        }
        let mut dialog = rfd::FileDialog::new().add_filter(
            self.t().text(Message::SourceDialogTitle),
            &[
                "iso", "img", "raw", "xz", "gz", "gzip", "zst", "zstd", "bz2", "bzip2",
            ],
        );
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        let Some(path) = dialog.pick_file() else {
            self.status = self.t().text(Message::StatusImageCancelled).into();
            return;
        };
        self.inspect_image_path(path);
    }

    fn inspect_image_path(&mut self, path: PathBuf) {
        if self.image_loading {
            self.status = self.t().text(Message::StatusImageBusy).into();
            return;
        }
        self.image_loading = true;
        self.status = self.t().text(Message::StatusImageInspecting).into();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = match Bootable::native().inspect_image(&path) {
                Ok(report) => Ok((report, path)),
                Err(error) => Err((error.to_string(), path)),
            };
            let _ = sender.send(result);
        });
        self.image_receiver = Some(receiver);
    }

    fn poll_image(&mut self) {
        let Some(receiver) = self.image_receiver.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok((image, _))) => {
                self.image_loading = false;
                self.status = self
                    .t()
                    .format(Message::StatusImageRecognized, &[("kind", &image.kind)]);
                self.remember_image(&image);
                self.reset_image_scoped_options();
                self.image = Some(image);
                self.advanced = false;
            }
            Ok(Err((error, path))) => {
                self.image_loading = false;
                self.preferences.forget_image(&path);
                self.save_preferences();
                self.reset_image_scoped_options();
                self.image = None;
                self.advanced = false;
                self.status = error;
            }
            Err(mpsc::TryRecvError::Empty) => self.image_receiver = Some(receiver),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.image_loading = false;
                self.status = self.t().text(Message::StatusImageStopped).into();
            }
        }
    }

    fn choose_folder(&mut self) {
        let mut dialog = rfd::FileDialog::new();
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        if let Some(directory) = dialog.pick_folder() {
            self.status = self.t().format(
                Message::StatusImageFolder,
                &[("path", &directory.display())],
            );
            self.browse_directory = Some(directory);
        } else {
            self.status = self.t().text(Message::StatusImageFolderCancelled).into();
        }
    }

    fn move_target_selection(&mut self, direction: i8) {
        let eligible = self
            .devices
            .iter()
            .enumerate()
            .filter_map(|(index, device)| device.is_eligible_target().then_some(index))
            .collect::<Vec<_>>();
        if eligible.is_empty() {
            self.selected = None;
            self.status = self.t().text(Message::StatusTargetNoneEligible).into();
            return;
        }
        let position = self
            .selected
            .and_then(|selected| eligible.iter().position(|index| *index == selected));
        let next = match (position, direction.is_negative()) {
            (None, _) => 0,
            (Some(position), true) => position.saturating_sub(1),
            (Some(position), false) => (position + 1).min(eligible.len() - 1),
        };
        self.selected = eligible.get(next).copied();
        self.workspace_focus = WorkspaceFocus::Target;
        self.status = self.t().text(Message::StatusTargetSelected).into();
    }

    fn move_workspace_focus(&mut self, backwards: bool) {
        self.workspace_focus = if backwards {
            self.workspace_focus.previous(self.image.is_some())
        } else {
            self.workspace_focus.next(self.image.is_some())
        };
        self.status = self
            .t()
            .text(match self.workspace_focus {
                WorkspaceFocus::Source => Message::FocusSource,
                WorkspaceFocus::Target => Message::FocusTarget,
                WorkspaceFocus::Setup => Message::FocusSetup,
                WorkspaceFocus::Review => Message::FocusReview,
                WorkspaceFocus::Discover => Message::FocusDiscover,
                WorkspaceFocus::Refresh => Message::FocusRefresh,
            })
            .into();
    }

    fn activate_workspace_focus(&mut self) {
        match self.workspace_focus {
            WorkspaceFocus::Source => self.choose_image(),
            WorkspaceFocus::Target => self.move_target_selection(1),
            WorkspaceFocus::Setup => self.toggle_advanced(),
            WorkspaceFocus::Review => self.preview(),
            WorkspaceFocus::Discover => self.toggle_catalog(),
            WorkspaceFocus::Refresh => self.refresh(true),
        }
    }

    fn refresh(&mut self, manual: bool) {
        if self.write_session.active() {
            if manual {
                self.status = self.t().text(Message::StatusDrivesRefreshPaused).into();
            }
            return;
        }
        match self.engine.discover_devices() {
            Ok(devices) => {
                if devices == self.devices {
                    if manual {
                        self.status = self.t().text(Message::StatusDrivesUpToDate).into();
                    }
                    return;
                }
                let added = devices
                    .iter()
                    .filter(|device| !self.devices.iter().any(|current| current.id == device.id))
                    .count();
                let removed = self
                    .devices
                    .iter()
                    .filter(|device| !devices.iter().any(|current| current.id == device.id))
                    .count();
                let selected_id = self
                    .selected
                    .and_then(|index| self.devices.get(index))
                    .map(|device| device.id.clone());
                self.selected =
                    selected_id.and_then(|id| devices.iter().position(|device| device.id == id));
                self.devices = devices;
                self.status = device_change_message(self.t(), added, removed);
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    fn preview(&mut self) {
        let Some(image) = self.image.clone() else {
            self.status = self.t().text(Message::StatusImageChooseFirst).into();
            return;
        };
        let Some(target) = self
            .selected
            .and_then(|index| self.devices.get(index))
            .cloned()
        else {
            self.status = self.t().text(Message::StatusTargetChooseFirst).into();
            return;
        };
        match self
            .engine
            .plan_with_options(image, target, self.options.clone())
        {
            Ok(plan) => {
                self.catalog_open = false;
                self.status = self.t().text(Message::StatusReviewOpen).into();
                self.write_session.open(plan);
                self.write_receiver = None;
            }
            Err(error) => {
                self.write_session.close();
                self.status = error.to_string();
            }
        }
    }

    fn review_readiness(&self) -> ReviewReadiness {
        review_readiness(
            self.image.as_ref(),
            self.selected.and_then(|index| self.devices.get(index)),
        )
    }

    fn close_review(&mut self) {
        if !self.write_session.close() {
            self.status = self.t().text(Message::StatusWriteActive).into();
            return;
        }
        self.status = self.review_readiness().guidance_in(self.locale).into();
    }

    fn open_write_confirmation(&mut self) {
        if self.write_session.open_confirmation() {
            self.status = self.t().text(Message::StatusReviewConsequences).into();
        }
    }

    fn close_write_confirmation(&mut self) {
        self.write_session.close_confirmation();
        self.status = self.t().text(Message::StatusWriteCancelled).into();
    }

    fn start_write(&mut self) {
        let launch = match self.write_session.begin() {
            Ok(launch) => launch,
            Err(message) => {
                self.status = message.into();
                return;
            }
        };
        self.status = self.t().text(Message::StatusWriteStarted).into();

        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let progress_sender = sender.clone();
            let completion =
                WriteCompletion::from_result(Bootable::native().write_with_privilege_controlled(
                    &launch.plan,
                    &launch.confirmation,
                    &launch.control,
                    move |progress| {
                        let _ = progress_sender.send(WriteUpdate::Progress(progress));
                    },
                ));
            let _ = sender.send(WriteUpdate::Finished(completion));
        });
        self.write_receiver = Some(receiver);
    }

    fn poll_write(&mut self) {
        let Some(receiver) = self.write_receiver.take() else {
            return;
        };
        let mut finished = false;
        while let Ok(update) = receiver.try_recv() {
            match update {
                WriteUpdate::Progress(progress) => {
                    self.status = self.write_session.apply_progress(progress);
                }
                WriteUpdate::Finished(completion) => {
                    finished = true;
                    let status = completion.status_in(self.locale);
                    self.write_session.finish(completion);
                    self.status = status;
                }
            }
        }
        if !finished {
            self.write_receiver = Some(receiver);
        }
    }

    fn cancel_write(&mut self) {
        if self.write_session.cancel() {
            self.status = self.t().text(Message::StatusWriteStopping).into();
        }
    }

    fn toggle_windows_requirements(&mut self) {
        let Some(image) = &self.image else {
            self.status = self.t().text(Message::StatusWindowsChooseInstaller).into();
            return;
        };
        if !matches!(
            image.kind,
            bootable_core::ImageKind::WindowsInstaller { .. }
        ) {
            self.status = self.t().text(Message::StatusWindowsNotWindows).into();
            return;
        }
        let enabled = !self.options.windows.bypass_hardware_requirements;
        self.options.windows.bypass_hardware_requirements = enabled;
        self.status = self
            .t()
            .text(if enabled {
                Message::OptionsWindowsBypassHardwareOn
            } else {
                Message::OptionsWindowsBypassHardwareOff
            })
            .into();
    }

    fn toggle_windows_offline_account(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        let enabled = !self.options.windows.allow_offline_account;
        self.options.windows.allow_offline_account = enabled;
        self.status = self
            .t()
            .text(if enabled {
                Message::OptionsWindowsOfflineAccountOn
            } else {
                Message::OptionsWindowsOfflineAccountOff
            })
            .into();
    }

    fn toggle_windows_privacy(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        let enabled = !self.options.windows.minimize_data_collection;
        self.options.windows.minimize_data_collection = enabled;
        self.status = self
            .t()
            .text(if enabled {
                Message::OptionsWindowsPrivacyOn
            } else {
                Message::OptionsWindowsPrivacyOff
            })
            .into();
    }

    fn toggle_windows_bitlocker(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        let enabled = !self.options.windows.disable_bitlocker;
        self.options.windows.disable_bitlocker = enabled;
        self.status = self
            .t()
            .text(if enabled {
                Message::OptionsWindowsBitlockerOn
            } else {
                Message::OptionsWindowsBitlockerOff
            })
            .into();
    }

    fn toggle_windows_named_account(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        if self.options.windows.local_account.is_some() {
            self.options.windows.local_account = None;
            self.status = self.t().text(Message::OptionsWindowsNamedAccountOff).into();
        } else {
            let account = bootable_core::suggested_account_name().unwrap_or_else(|| "User".into());
            self.options.windows.local_account = Some(account.clone());
            self.options.windows.allow_offline_account = true;
            self.status = self.t().format(
                Message::OptionsWindowsNamedAccountOn,
                &[("account", &account)],
            );
        }
    }

    fn toggle_windows_regional(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        if self.options.windows.regional.is_some() {
            self.options.windows.regional = None;
            self.status = self.t().text(Message::OptionsWindowsHostRegionOff).into();
        } else {
            let regional = bootable_core::host_regional_options();
            self.status = self.t().format(
                Message::OptionsWindowsHostRegionOn,
                &[
                    ("locale", &regional.user_locale),
                    ("zone", &regional.time_zone),
                ],
            );
            self.options.windows.regional = Some(regional);
        }
    }

    fn toggle_windows_qol(&mut self) {
        if self.windows_options_available() {
            let enabled = !self.options.windows.quality_of_life;
            self.options.windows.quality_of_life = enabled;
            self.status = self
                .t()
                .text(if enabled {
                    Message::OptionsWindowsQolOn
                } else {
                    Message::OptionsWindowsQolOff
                })
                .into();
        }
    }

    fn toggle_windows_ca_2023(&mut self) {
        if self.windows_options_available() {
            let enabled = !self.options.windows.use_windows_ca_2023;
            self.options.windows.use_windows_ca_2023 = enabled;
            self.status = self
                .t()
                .text(if enabled {
                    Message::OptionsWindowsCa2023On
                } else {
                    Message::OptionsWindowsCa2023Off
                })
                .into();
        }
    }

    fn toggle_windows_skusi_policy(&mut self) {
        if self.windows_options_available() {
            let enabled = !self.options.windows.apply_skusi_policy;
            self.options.windows.apply_skusi_policy = enabled;
            self.status = self
                .t()
                .text(if enabled {
                    Message::OptionsWindowsSkusipolicyOn
                } else {
                    Message::OptionsWindowsSkusipolicyOff
                })
                .into();
        }
    }

    fn toggle_windows_s_mode(&mut self) {
        if self.windows_options_available() {
            let enabled = !self.options.windows.force_s_mode;
            self.options.windows.force_s_mode = enabled;
            self.status = self
                .t()
                .text(if enabled {
                    Message::OptionsWindowsSmodeOn
                } else {
                    Message::OptionsWindowsSmodeOff
                })
                .into();
        }
    }

    fn cycle_windows_partition_scheme(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        cycle_partition_scheme(&mut self.options);
        self.status = self.t().format(
            Message::StatusWindowsScheme,
            &[
                ("scheme", &self.options.windows_partition_scheme),
                ("firmware", &self.options.windows_boot_firmware),
            ],
        );
    }

    fn cycle_windows_boot_firmware(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        cycle_boot_firmware(&mut self.options);
        self.status = self.t().format(
            Message::StatusWindowsFirmware,
            &[
                ("firmware", &self.options.windows_boot_firmware),
                ("scheme", &self.options.windows_partition_scheme),
            ],
        );
    }

    /// Destructive or advanced choices tied to one image are never carried to
    /// the next image or persisted in preferences.
    fn reset_image_scoped_options(&mut self) {
        self.options.windows_boot_firmware = bootable_core::WindowsBootFirmware::default();
    }

    fn windows_options_available(&mut self) -> bool {
        let available = self.image.as_ref().is_some_and(|image| {
            matches!(
                image.kind,
                bootable_core::ImageKind::WindowsInstaller { .. }
            )
        });
        if !available {
            self.status = self.t().text(Message::StatusWindowsChooseInstaller).into();
        }
        available
    }

    fn toggle_advanced(&mut self) {
        if self.image.is_none() {
            self.advanced = false;
            self.status = self.t().text(Message::StatusOptionsOpenNeedsImage).into();
            return;
        }
        self.advanced = !self.advanced;
        self.status = self
            .t()
            .text(if self.advanced {
                Message::StatusOptionsExpanded
            } else {
                Message::StatusOptionsCollapsed
            })
            .into();
    }

    fn cycle_checksum_algorithm(&mut self) {
        self.checksum_algorithm = self.checksum_algorithm.next();
        self.status = self.t().format(
            Message::StatusChecksumAlgorithm,
            &[("algorithm", &self.checksum_algorithm)],
        );
        self.preferences.checksum_algorithm = self.checksum_algorithm;
        self.save_preferences();
    }

    fn cycle_bad_blocks(&mut self) {
        self.options.bad_block_check = self.options.bad_block_check.next();
        self.status = self.options.bad_block_check.status_in(self.locale);
    }

    fn checksum(&mut self) {
        let Some(image) = &self.image else {
            self.status = self.t().text(Message::StatusChecksumChooseImage).into();
            return;
        };
        self.status = match self
            .engine
            .checksum_image(&image.path, self.checksum_algorithm)
        {
            Ok(checksum) => format!("{}: {}", checksum.algorithm, checksum.hexadecimal),
            Err(error) => error.to_string(),
        };
    }

    fn backup(&mut self) {
        let Some(device) = self
            .selected
            .and_then(|index| self.devices.get(index))
            .cloned()
        else {
            self.status = self.t().text(Message::StatusBackupChooseDrive).into();
            return;
        };
        let mut dialog = rfd::FileDialog::new()
            .add_filter(
                self.t().text(Message::SourceDialogFilterBackup),
                &["img", "raw", "dd"],
            )
            .set_file_name("bootable-backup.img");
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        let Some(destination) = dialog.save_file() else {
            self.status = self.t().text(Message::StatusBackupCancelled).into();
            return;
        };
        self.status = self.t().format(
            Message::StatusBackupRunning,
            &[("drive", &device.display_name())],
        );
        let mut latest = self.status.clone();
        let result = self
            .engine
            .backup_device(device.id.as_str(), &destination, |progress| {
                latest = progress.message;
            });
        self.status = match result {
            Ok(()) => self.t().format(
                Message::StatusBackupDone,
                &[("path", &destination.display())],
            ),
            Err(error) => self.t().format(
                Message::StatusBackupFailed,
                &[("error", &error), ("step", &latest)],
            ),
        };
    }

    fn handle_write_flow_click(&mut self, point: (u16, u16)) -> Option<bool> {
        if contains(self.hit_regions.download_pause, point) {
            self.toggle_download_pause();
        } else if contains(self.hit_regions.download_cancel, point) {
            self.cancel_download();
        } else if contains(self.hit_regions.open_image, point) {
            self.choose_image();
        } else if let Some(index) = self
            .hit_regions
            .recent_rows
            .iter()
            .find(|(area, _)| area.contains(point.into()))
            .map(|(_, index)| *index)
        {
            self.use_recent_image(index);
        } else if contains(self.hit_regions.advanced, point) {
            self.toggle_advanced();
        } else if contains(self.hit_regions.choose_folder, point) {
            self.choose_folder();
        } else if contains(self.hit_regions.windows_options, point) {
            self.toggle_windows_requirements();
        } else if contains(self.hit_regions.windows_offline, point) {
            self.toggle_windows_offline_account();
        } else if contains(self.hit_regions.windows_privacy, point) {
            self.toggle_windows_privacy();
        } else if contains(self.hit_regions.windows_bitlocker, point) {
            self.toggle_windows_bitlocker();
        } else if contains(self.hit_regions.windows_named_account, point) {
            self.toggle_windows_named_account();
        } else if contains(self.hit_regions.windows_regional, point) {
            self.toggle_windows_regional();
        } else if contains(self.hit_regions.windows_qol, point) {
            self.toggle_windows_qol();
        } else if contains(self.hit_regions.windows_ca_2023, point) {
            self.toggle_windows_ca_2023();
        } else if contains(self.hit_regions.windows_skusi_policy, point) {
            self.toggle_windows_skusi_policy();
        } else if contains(self.hit_regions.windows_s_mode, point) {
            self.toggle_windows_s_mode();
        } else if contains(self.hit_regions.windows_partition_scheme, point) {
            self.cycle_windows_partition_scheme();
        } else if contains(self.hit_regions.windows_boot_firmware, point) {
            self.cycle_windows_boot_firmware();
        } else if contains(self.hit_regions.bad_blocks, point) {
            self.cycle_bad_blocks();
        } else if contains(self.hit_regions.checksum_algorithm, point) {
            self.cycle_checksum_algorithm();
        } else if contains(self.hit_regions.preview, point) {
            self.preview();
        } else if contains(self.hit_regions.checksum, point) {
            self.checksum();
        } else if contains(self.hit_regions.backup, point) {
            self.backup();
        } else if contains(self.hit_regions.quit, point) {
            return Some(true);
        } else {
            let index = self
                .hit_regions
                .device_rows
                .iter()
                .find(|(area, _)| area.contains(point.into()))
                .map(|(_, index)| *index)?;
            if self
                .devices
                .get(index)
                .is_some_and(Device::is_eligible_target)
            {
                self.selected = Some(index);
                self.workspace_focus = WorkspaceFocus::Target;
                self.status = self.t().text(Message::StatusTargetSelected).into();
            } else {
                self.status = self.t().text(Message::StatusTargetBlocked).into();
            }
        }
        Some(false)
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        if self.help_open {
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                self.help_open = false;
            }
            return false;
        }
        if self.write_session.confirmation_open() {
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                let point = (mouse.column, mouse.row);
                if contains(self.hit_regions.confirm_acknowledge, point) {
                    self.write_session.toggle_acknowledged();
                } else if contains(self.hit_regions.confirm_cancel, point) {
                    self.close_write_confirmation();
                } else if contains(self.hit_regions.confirm_write, point) {
                    self.start_write();
                }
            }
            return false;
        }
        if self.write_session.is_reviewing() {
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                let point = (mouse.column, mouse.row);
                if contains(self.hit_regions.language, point) {
                    self.cycle_language();
                } else if contains(self.hit_regions.review_back, point) {
                    self.close_review();
                } else if contains(self.hit_regions.review_write, point) {
                    if self.write_session.active() {
                        self.cancel_write();
                    } else {
                        self.open_write_confirmation();
                    }
                } else if contains(self.hit_regions.quit, point) {
                    if self.write_session.active() {
                        self.status = self.t().text(Message::StatusWriteActive).into();
                    } else {
                        return true;
                    }
                }
            }
            return false;
        }
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            let point = (mouse.column, mouse.row);
            if contains(self.hit_regions.guide, point) {
                self.toggle_help();
                return false;
            }
            if contains(self.hit_regions.language, point) {
                self.cycle_language();
                return false;
            }
            if contains(self.hit_regions.downloads, point) {
                self.toggle_downloads();
                return false;
            }
        }
        if self.downloads_open {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.download_selected = self.download_selected.saturating_sub(1);
                }
                MouseEventKind::ScrollDown => {
                    self.download_selected = (self.download_selected + 1)
                        .min(self.download_session.jobs().len().saturating_sub(1));
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    let point = (mouse.column, mouse.row);
                    if contains(self.hit_regions.download_retry, point) {
                        self.retry_selected_download();
                    } else if contains(self.hit_regions.download_use, point) {
                        self.use_selected_download();
                    } else if contains(self.hit_regions.download_remove, point) {
                        self.remove_selected_download();
                    } else if let Some((_, index)) = self
                        .hit_regions
                        .download_rows
                        .iter()
                        .find(|(area, _)| area.contains(point.into()))
                    {
                        self.download_selected = *index;
                    }
                }
                _ => {}
            }
            return false;
        }
        if self.catalog_open {
            match mouse.kind {
                MouseEventKind::ScrollUp => match self.catalog_focus {
                    CatalogFocus::Distributions => {
                        if self.discovery_session.source() == DiscoverySource::RaspberryPi {
                            self.pi_device_selected = self.pi_device_selected.saturating_sub(1);
                        } else {
                            self.move_distribution(-1);
                        }
                    }
                    CatalogFocus::Releases => {
                        if self.discovery_session.source() == DiscoverySource::RaspberryPi {
                            self.move_pi_image(-1);
                        } else {
                            self.release_selected = self.release_selected.saturating_sub(1);
                        }
                    }
                },
                MouseEventKind::ScrollDown => match self.catalog_focus {
                    CatalogFocus::Distributions => {
                        if self.discovery_session.source() == DiscoverySource::RaspberryPi {
                            let count = self
                                .pi_catalog
                                .as_ref()
                                .map(|catalog| catalog.devices.len())
                                .unwrap_or_default();
                            self.pi_device_selected =
                                (self.pi_device_selected + 1).min(count.saturating_sub(1));
                        } else {
                            self.move_distribution(1);
                        }
                    }
                    CatalogFocus::Releases => {
                        if self.discovery_session.source() == DiscoverySource::RaspberryPi {
                            self.move_pi_image(1);
                        } else {
                            self.release_selected = (self.release_selected + 1)
                                .min(self.catalog_releases.len().saturating_sub(1));
                        }
                    }
                },
                MouseEventKind::Down(MouseButton::Left) => {
                    let point = (mouse.column, mouse.row);
                    if contains(self.hit_regions.discover, point) {
                        self.toggle_catalog();
                    } else if contains(self.hit_regions.refresh, point) {
                        self.refresh(true);
                    } else if let Some(should_quit) = self.handle_write_flow_click(point) {
                        return should_quit;
                    } else if contains(self.hit_regions.catalog_close, point) {
                        self.toggle_catalog();
                    } else if contains(self.hit_regions.catalog_retry, point) {
                        self.retry_catalog();
                    } else if self.discovery_session.quick_access() != QuickAccess::Windows
                        && contains(self.hit_regions.catalog_search, point)
                    {
                        self.catalog_searching = true;
                        self.status = format!(
                            "{} · Esc",
                            self.t().text(Message::DiscoverSearchPlaceholder)
                        );
                    } else if contains(self.hit_regions.source_distrowatch, point) {
                        self.show_quick_access(QuickAccess::All);
                    } else if contains(self.hit_regions.source_arch, point) {
                        self.show_quick_access(QuickAccess::Arch);
                    } else if contains(self.hit_regions.source_debian, point) {
                        self.show_quick_access(QuickAccess::Debian);
                    } else if contains(self.hit_regions.source_omarchy, point) {
                        self.show_quick_access(QuickAccess::Omarchy);
                    } else if contains(self.hit_regions.source_windows, point) {
                        self.show_quick_access(QuickAccess::Windows);
                    } else if contains(self.hit_regions.source_raspberry_pi, point) {
                        self.load_raspberry_pi();
                    } else if contains(self.hit_regions.catalog_download, point) {
                        if self.discovery_session.quick_access() == QuickAccess::Windows {
                            self.choose_image();
                        } else {
                            match self.discovery_session.source() {
                                DiscoverySource::DistroWatch
                                    if self.catalog_releases.is_empty() =>
                                {
                                    self.open_selected_distrowatch_page()
                                }
                                DiscoverySource::DistroWatch => self.download_catalog_release(),
                                DiscoverySource::RaspberryPi => self.download_pi_catalog_image(),
                            }
                        }
                    } else if let Some((_, index)) = self
                        .hit_regions
                        .pi_device_rows
                        .iter()
                        .find(|(area, _)| area.contains(point.into()))
                    {
                        self.pi_device_selected = *index;
                        self.catalog_focus = CatalogFocus::Distributions;
                        self.select_first_pi_image_for_device();
                    } else if let Some((_, index)) = self
                        .hit_regions
                        .pi_image_rows
                        .iter()
                        .find(|(area, _)| area.contains(point.into()))
                    {
                        self.pi_image_selected = *index;
                        self.catalog_focus = CatalogFocus::Releases;
                        self.status = self.t().text(Message::StatusCatalogPiSelectedVerify).into();
                    } else if let Some((_, index)) = self
                        .hit_regions
                        .distribution_rows
                        .iter()
                        .find(|(area, _)| area.contains(point.into()))
                    {
                        let index = *index;
                        self.catalog_focus = CatalogFocus::Distributions;
                        self.select_catalog_distribution(index);
                    } else if let Some((_, index)) = self
                        .hit_regions
                        .release_rows
                        .iter()
                        .find(|(area, _)| area.contains(point.into()))
                    {
                        self.catalog_focus = CatalogFocus::Releases;
                        self.release_selected = *index;
                        self.status = self.iso_selected_status();
                    }
                }
                _ => {}
            }
            return false;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.move_target_selection(-1);
            }
            MouseEventKind::ScrollDown => {
                self.move_target_selection(1);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let point = (mouse.column, mouse.row);
                if contains(self.hit_regions.discover, point) {
                    self.toggle_catalog();
                } else if contains(self.hit_regions.refresh, point) {
                    self.refresh(true);
                } else if let Some(should_quit) = self.handle_write_flow_click(point) {
                    return should_quit;
                }
            }
            _ => {}
        }
        false
    }
}

fn run_tui(engine: Bootable, image_path: Option<PathBuf>) -> Result<()> {
    enable_raw_mode().context("enable raw terminal mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("enter alternate screen and enable mouse capture")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    let artwork_picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    let mut app = App::load(engine, image_path, artwork_picker);

    let result = event_loop(&mut terminal, &mut app);
    disable_raw_mode().context("disable raw terminal mode")?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )
    .context("disable mouse capture and leave alternate screen")?;
    terminal.show_cursor().context("show cursor")?;
    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    let mut last_device_scan = Instant::now();
    let mut last_download_scan = Instant::now();
    let mut state_started = false;
    loop {
        app.poll_download();
        app.poll_write();
        app.poll_image();
        app.poll_catalog();
        app.sync_catalog_artwork();
        terminal.draw(|frame| draw(frame, app))?;
        if !state_started {
            app.refresh_download_jobs();
            app.start_next_queued_download();
            if let Some(path) = app.initial_image.take() {
                app.inspect_image_path(path);
            }
            state_started = true;
        }
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.help_open {
                        if matches!(
                            key.code,
                            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?' | 'q')
                        ) {
                            app.help_open = false;
                        }
                        continue;
                    }
                    if key.code == KeyCode::Char('?')
                        && !app.write_session.confirmation_open()
                        && !app.catalog_searching
                    {
                        app.toggle_help();
                        continue;
                    }
                    if key.code == KeyCode::Char('L')
                        && !app.write_session.confirmation_open()
                        && !app.catalog_searching
                    {
                        app.cycle_language();
                        continue;
                    }
                    if app.download_session.is_active() {
                        match key.code {
                            KeyCode::Char('p') => {
                                app.toggle_download_pause();
                                continue;
                            }
                            KeyCode::Char('x' | 'q') | KeyCode::Esc => {
                                app.cancel_download();
                                continue;
                            }
                            _ => {}
                        }
                    }
                    if app.write_session.is_reviewing() {
                        if app.write_session.confirmation_open() {
                            match key.code {
                                KeyCode::Esc => app.close_write_confirmation(),
                                KeyCode::Char(' ') => {
                                    app.write_session.toggle_acknowledged();
                                }
                                KeyCode::Enter if app.write_session.acknowledged() => {
                                    app.start_write()
                                }
                                KeyCode::Enter => {
                                    app.status =
                                        app.t().text(Message::StatusReviewAckRequired).into();
                                }
                                _ => {}
                            }
                            continue;
                        }
                        match key.code {
                            KeyCode::Char('q' | 'c')
                                if key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !app.write_session.active() =>
                            {
                                return Ok(());
                            }
                            KeyCode::Esc if !app.write_session.active() => app.close_review(),
                            KeyCode::Char('x' | 'c') if app.write_session.active() => {
                                app.cancel_write();
                            }
                            KeyCode::Enter
                                if !app.write_session.active()
                                    && !app.write_session.succeeded() =>
                            {
                                app.open_write_confirmation();
                            }
                            _ if app.write_session.active() => {
                                // The shared line has no key legend; the TUI appends its own.
                                app.status =
                                    format!("{} (x)", app.t().text(Message::StatusWriteActive));
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if key.code == KeyCode::Char('q') {
                        return Ok(());
                    }
                    if app.downloads_open {
                        app.handle_download_key(key.code);
                        continue;
                    }
                    if key.code == KeyCode::Char('m') {
                        app.toggle_downloads();
                        continue;
                    }
                    if !app.catalog_open {
                        match key.code {
                            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                                app.move_workspace_focus(true);
                                continue;
                            }
                            KeyCode::Tab => {
                                app.move_workspace_focus(false);
                                continue;
                            }
                            KeyCode::BackTab => {
                                app.move_workspace_focus(true);
                                continue;
                            }
                            KeyCode::Enter => {
                                app.activate_workspace_focus();
                                continue;
                            }
                            _ => {}
                        }
                    }
                    match key.code {
                        code if app.catalog_open => app.handle_catalog_key(code),
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc => return Ok(()),
                        KeyCode::Char('o') => app.choose_image(),
                        KeyCode::Char('g') => app.toggle_catalog(),
                        KeyCode::Char('d') => app.choose_folder(),
                        KeyCode::Char('a') => app.toggle_advanced(),
                        KeyCode::Char('w') => app.toggle_windows_requirements(),
                        KeyCode::Char('n') => app.toggle_windows_offline_account(),
                        KeyCode::Char('v') => app.toggle_windows_privacy(),
                        KeyCode::Char('l') => app.toggle_windows_bitlocker(),
                        KeyCode::Char('b') => app.cycle_bad_blocks(),
                        KeyCode::Char('c') => app.cycle_checksum_algorithm(),
                        KeyCode::Char('r') => app.refresh(true),
                        KeyCode::Char('p') => app.preview(),
                        KeyCode::Char('h') => app.checksum(),
                        KeyCode::Char('u') => app.backup(),
                        KeyCode::Char(digit @ '1'..='4') => {
                            app.use_recent_image(usize::from(digit as u8 - b'1'));
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            app.move_target_selection(-1);
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            app.move_target_selection(1);
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) if app.handle_mouse(mouse) => return Ok(()),
                _ => {}
            }
        }
        if last_device_scan.elapsed() >= DEVICE_SCAN_INTERVAL {
            app.refresh(false);
            last_device_scan = Instant::now();
        }
        if last_download_scan.elapsed() >= DOWNLOAD_SCAN_INTERVAL {
            app.refresh_download_jobs();
            app.start_next_queued_download();
            last_download_scan = Instant::now();
        }
    }
}

fn draw(frame: &mut ratatui::Frame<'_>, app: &mut App) {
    draw_screen(frame, app);
    if app.help_open && !app.write_session.confirmation_open() {
        draw_help(frame, frame.area(), app.locale);
    }
}

fn draw_help(frame: &mut ratatui::Frame<'_>, area: Rect, locale: Locale) {
    let width = area.width.saturating_sub(4).min(110);
    let text_width = usize::from(width.saturating_sub(2));
    let sections = help_sections(locale);
    let key_style = Style::default().fg(Color::Rgb(229, 185, 95));
    let white = Style::default().fg(Color::White);
    let muted = Style::default().fg(MUTED);
    // Key chords are never translated, but size the column from the data
    // rather than assuming the English widths.
    let key_column = sections
        .iter()
        .flat_map(|section| &section.entries)
        .map(|entry| display_width(entry.terminal) + 2)
        .max()
        .unwrap_or(0)
        .max(12);
    let indent = 2 + key_column;
    let mut lines = wrap_styled(&[(help_intro(locale), white)], text_width)
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    lines.push(Line::raw(""));
    for section in &sections {
        lines.push(Line::from(Span::styled(
            section.title.to_uppercase(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        for entry in &section.entries {
            let detail = format!(" · {}", entry.detail);
            let body = wrap_styled(
                &[(entry.action, white), (detail.as_str(), muted)],
                text_width.saturating_sub(indent),
            );
            for (index, mut spans) in body.into_iter().enumerate() {
                let prefix = if index == 0 {
                    Span::styled(
                        format!("  {}", pad_display(entry.terminal, key_column)),
                        key_style,
                    )
                } else {
                    Span::raw(" ".repeat(indent))
                };
                spans.insert(0, prefix);
                lines.push(Line::from(spans));
            }
        }
    }
    lines.push(Line::raw(""));
    lines.extend(
        wrap_styled(&[("Press Esc, ? or click to close", muted)], text_width)
            .into_iter()
            .map(Line::from),
    );
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let modal = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, modal);
    let guide_title = format!(" {} ", locale.strings().text(Message::GuideTitle));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel_block(&guide_title).style(Style::default().bg(PANEL))),
        modal,
    );
}

fn draw_screen(frame: &mut ratatui::Frame<'_>, app: &mut App) {
    app.hit_regions = HitRegions::default();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG)),
        frame.area(),
    );
    let canvas = application_area(frame.area());
    if canvas.width < 44 || canvas.height < 22 {
        draw_terminal_too_small(frame, canvas);
        return;
    }
    if app.write_session.is_reviewing() {
        draw_review(frame, app, canvas);
        return;
    }
    let show_options = app.image.is_some() && app.advanced;
    let setup_available = app.image.is_some() && !app.advanced;
    let header_height = 5;
    let status_height = 7;
    let options_height = if show_options {
        advanced_height(canvas.width)
    } else {
        0
    };
    let workspace_height = workspace_height(canvas.width);
    let shell = main_shell_layout(canvas, header_height, status_height);
    draw_header(frame, app, shell[0]);
    draw_status(frame, app, shell[2]);
    let content = shell[1];

    if app.downloads_open {
        draw_download_manager(frame, app, content);
        return;
    }

    if app.catalog_open {
        let required = catalog_min_height(canvas.width)
            .saturating_add(workspace_height)
            .saturating_add(options_height)
            .saturating_add(if show_options { 2 } else { 1 });
        if content.height >= required {
            let mut constraints = vec![Constraint::Length(workspace_height)];
            if show_options {
                constraints.push(Constraint::Length(options_height));
            }
            constraints.push(Constraint::Min(catalog_min_height(canvas.width)));
            let rows = Layout::vertical(constraints).spacing(1).split(content);
            draw_workspace(frame, app, rows[0]);
            if show_options {
                draw_advanced(frame, app, rows[1]);
            }
            draw_catalog(frame, app, rows[usize::from(show_options) + 1]);
        } else {
            draw_catalog(frame, app, content);
        }
        return;
    }

    if show_options && content.height < workspace_height.saturating_add(options_height + 1) {
        // Too short for the workspace and the options together: the options
        // the user asked for take the whole content area.
        draw_advanced(frame, app, content);
        return;
    }

    let show_setup_toggle = setup_available && content.height >= workspace_height.saturating_add(4);
    let show_discovery_toggle = content.height
        >= workspace_height
            .saturating_add(if show_setup_toggle { 4 } else { 0 })
            .saturating_add(4);
    let mut constraints = vec![Constraint::Length(workspace_height)];
    if show_options {
        constraints.push(Constraint::Length(options_height));
    } else if show_setup_toggle {
        constraints.push(Constraint::Length(3));
    }
    if show_discovery_toggle {
        constraints.push(Constraint::Length(3));
    }
    constraints.push(Constraint::Min(0));
    let rows = Layout::vertical(constraints).spacing(1).split(content);
    draw_workspace(frame, app, rows[0]);
    let mut next_row = 1;
    if show_options {
        draw_advanced(frame, app, rows[next_row]);
        next_row += 1;
    } else if show_setup_toggle {
        draw_collapsed_setup(frame, app, rows[next_row]);
        next_row += 1;
    }
    if show_discovery_toggle {
        draw_collapsed_discovery(frame, app, rows[next_row]);
    }
}

fn draw_collapsed_setup(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let t = app.t();
    let summary = t.format(
        Message::OptionsSummaryVerification,
        &[(
            "bad_blocks",
            &app.options.bad_block_check.label_in(app.locale),
        )],
    );
    render_button(
        frame,
        area,
        &format!("+  {} · {summary}", t.text(Message::ActionSetupOptions)),
        app.workspace_focus == WorkspaceFocus::Setup,
    );
    app.hit_regions.advanced = Some(area);
}

fn draw_collapsed_discovery(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    render_button(
        frame,
        area,
        &format!(
            "+  {} · {} →",
            app.t().text(Message::ActionDiscover),
            app.t().text(Message::DiscoverCollapsedHint)
        ),
        app.workspace_focus == WorkspaceFocus::Discover,
    );
    app.hit_regions.discover = Some(area);
}

fn draw_review(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let Some(plan) = app.write_session.plan() else {
        return;
    };
    let compact = area.height < 30;
    let show_steps = !compact;
    let show_result = app.write_session.completion().is_some();
    let show_progress = app.write_session.progress().is_some() && (!compact || !show_result);
    let show_confirmation = !compact || (!app.write_session.active() && !show_result);
    let write_succeeded = app.write_session.succeeded();
    let source = format!(
        "{} • {} • {}",
        plan.image.path.display(),
        plan.image.kind,
        format_bytes(plan.image.size)
    );
    let target = format!(
        "{} • {} • {}",
        plan.target.path.display(),
        plan.target.display_name(),
        format_bytes(plan.target.capacity)
    );
    let method = plan.strategy.to_string();
    let t = app.t();
    let steps = plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let marker = if step.destructive {
                t.heading(Message::ReviewStepErases)
            } else {
                t.text(Message::ReviewStepSafe).to_string()
            };
            ListItem::new(format!("{}. {}  ·  {marker}", index + 1, step.title)).style(
                Style::default().fg(if step.destructive {
                    Color::Yellow
                } else {
                    Color::White
                }),
            )
        })
        .collect::<Vec<_>>();

    let panel_text_width = usize::from(area.width.saturating_sub(2));
    let mut permanent_lines = wrapped_lines(
        t.text(Message::ReviewConsequence),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        panel_text_width,
    );
    if app.write_session.active() {
        permanent_lines.extend(wrapped_lines(
            t.text(Message::ReviewWarningWriting),
            Style::default().fg(Color::Yellow),
            panel_text_width,
        ));
    } else {
        for message in [Message::ReviewSubtitle, Message::ReviewHintOpenConfirmation] {
            permanent_lines.extend(wrapped_lines(
                t.text(message),
                Style::default().fg(MUTED),
                panel_text_width,
            ));
        }
    }
    // Longer wording (German, Russian) wraps onto more lines; give the panel
    // the rows it needs instead of clipping a safety sentence.
    let permanent_height = (permanent_lines.len() as u16 + 2).clamp(6, if compact { 7 } else { 9 });
    let mut constraints = vec![
        Constraint::Length(if compact { 3 } else { 4 }),
        Constraint::Length(if compact { 5 } else { 6 }),
    ];
    if show_steps {
        constraints.push(Constraint::Min(4));
    } else {
        constraints.push(Constraint::Min(0));
    }
    if show_confirmation {
        constraints.push(Constraint::Length(permanent_height));
    }
    if show_progress {
        constraints.push(Constraint::Length(5));
    }
    if show_result {
        constraints.push(Constraint::Length(4));
    }
    constraints.push(Constraint::Length(3));
    let rows = Layout::vertical(constraints).spacing(1).split(area);
    let mut row = 0;
    {
        // The brand lockup uses at most two lines; the last line of the
        // header carries the language hint.
        let header = rows[row];
        let hint_row = Rect::new(header.x, header.bottom().saturating_sub(1), header.width, 1);
        draw_language_hint(frame, app, hint_row, usize::from(header.width));
    }
    frame.render_widget(
        Paragraph::new(brand_lockup(
            area.width >= 60,
            t.text(Message::ReviewTitle),
            t.text(if app.write_session.active() {
                Message::ReviewSubtitleWriting
            } else {
                Message::HeaderSubtitleReview
            }),
            t.text(Message::HeaderTagline),
            usize::from(rows[row].width),
        ))
        .style(Style::default().bg(BG)),
        rows[row],
    );
    row += 1;
    let summary_labels = [
        t.heading(Message::ReviewFieldSource),
        t.heading(Message::ReviewFieldTarget),
        t.heading(Message::ReviewFieldMethod),
    ];
    let label_width = summary_labels
        .iter()
        .map(|label| display_width(label))
        .max()
        .unwrap_or(0)
        + 2;
    let summary_label = |index: usize| {
        Span::styled(
            pad_display(&summary_labels[index], label_width),
            Style::default().fg(MUTED),
        )
    };
    let plan_summary_title = format!(" {} ", t.text(Message::ReviewPlanSummary));
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                summary_label(0),
                Span::styled(source, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                summary_label(1),
                Span::styled(target, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                summary_label(2),
                Span::styled(method, Style::default().fg(ACCENT)),
            ]),
        ])
        .wrap(Wrap { trim: true })
        .block(panel_block(&plan_summary_title)),
        rows[row],
    );
    row += 1;
    if show_steps {
        let operations_title = format!(" {} ", t.text(Message::ReviewOrderedOperations));
        frame.render_widget(
            List::new(steps).block(panel_block(&operations_title)),
            rows[row],
        );
    }
    row += 1;
    if show_confirmation {
        let permanent_title = format!(" {} ", t.text(Message::ReviewPermanentChanges));
        frame.render_widget(
            Paragraph::new(permanent_lines).block(panel_block(&permanent_title)),
            rows[row],
        );
        row += 1;
    }
    if let Some(progress) = app.write_session.progress() {
        let elapsed = app
            .write_session
            .started_at()
            .map(|started| started.elapsed())
            .unwrap_or_default();
        let progress_title = format!(
            " {} · {} ",
            progress.phase.label_in(app.locale),
            progress.message
        );
        frame.render_widget(
            Gauge::default()
                .block(panel_block(&progress_title))
                .gauge_style(
                    Style::default()
                        .fg(if write_succeeded {
                            ACCENT
                        } else {
                            Color::Yellow
                        })
                        .bg(PANEL_SOFT),
                )
                .ratio(progress.ratio().unwrap_or_default())
                .label(progress.metrics(elapsed)),
            rows[row],
        );
        row += 1;
    }
    if let Some(completion) = app.write_session.completion() {
        let color = match completion {
            WriteCompletion::Succeeded => ACCENT,
            WriteCompletion::AuthenticationDenied => Color::Yellow,
            WriteCompletion::Cancelled | WriteCompletion::Failed(_) => Color::LightRed,
        };
        let result_title = format!(" {} ", completion.title_in(app.locale));
        frame.render_widget(
            Paragraph::new(wrapped_lines(
                &completion.detail_in(app.locale),
                Style::default().fg(color),
                usize::from(rows[row].width.saturating_sub(2)),
            ))
            .block(panel_block(&result_title)),
            rows[row],
        );
        row += 1;
    }
    let actions = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .spacing(1)
    .split(rows[row]);
    let back_label = format!("←  {}", t.text(Message::ActionBack));
    if app.write_session.active() {
        render_disabled_button(frame, actions[0], &back_label);
    } else {
        render_button(frame, actions[0], &back_label, false);
    }
    let write_enabled = !write_succeeded;
    let write_label = if app.write_session.active() {
        format!("■  {}", t.text(Message::ReviewActionStopSafely))
    } else if write_succeeded {
        format!("✓  {}", t.text(Message::ReviewActionWritten))
    } else if app.write_session.completion().is_some() {
        format!("!  {}", t.text(Message::ReviewActionRetry))
    } else {
        format!("!  {}", t.text(Message::ReviewActionConsequences))
    };
    if write_enabled {
        render_button(frame, actions[1], &write_label, true);
    } else {
        render_disabled_button(frame, actions[1], &write_label);
    }
    let quit_label = format!("×  {}", t.text(Message::ActionQuit));
    if app.write_session.active() {
        render_disabled_button(frame, actions[2], &quit_label);
    } else {
        render_button(frame, actions[2], &quit_label, false);
    }
    app.hit_regions.review_back = (!app.write_session.active()).then_some(actions[0]);
    app.hit_regions.review_write = write_enabled.then_some(actions[1]);
    app.hit_regions.quit = (!app.write_session.active()).then_some(actions[2]);
    if app.write_session.confirmation_open() {
        draw_write_confirmation_modal(frame, app, area);
    }
}

fn draw_write_confirmation_modal(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let t = app.t();
    let Some(plan) = app.write_session.plan() else {
        return;
    };
    let width = area.width.saturating_sub(4).min(100);
    let height = area.height.saturating_sub(2).min(32);
    let modal = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let inner = modal.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    // Text columns inside a bordered panel in the modal.
    let text_width = usize::from(inner.width.saturating_sub(2));
    let target = format!(
        "{} • {}\n{} • {}",
        plan.target.display_name(),
        format_bytes(plan.target.capacity),
        plan.target.path.display(),
        plan.strategy
    );
    let changes = plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let marker = if step.destructive {
                t.heading(Message::ReviewStepErases)
            } else {
                t.text(Message::ReviewStepVerifies).to_string()
            };
            ListItem::new(format!("{}. {}  ·  {marker}", index + 1, step.title)).style(
                Style::default().fg(if step.destructive {
                    Color::LightRed
                } else {
                    Color::White
                }),
            )
        })
        .collect::<Vec<_>>();
    let bullets = [
        (Message::ConfirmConsequenceErase, Color::LightRed),
        (Message::ConfirmConsequenceWrongDrive, Color::Yellow),
        (Message::ConfirmConsequenceInterrupted, Color::Yellow),
        (Message::ConfirmConsequenceRecheck, MUTED),
    ];
    let bullet_lines = |count: usize| {
        bullets
            .iter()
            .take(count)
            .flat_map(|(message, color)| {
                wrapped_lines(
                    &format!("• {}", t.text(*message)),
                    Style::default().fg(*color),
                    text_width,
                )
            })
            .collect::<Vec<_>>()
    };
    let bullet_height = |count: usize| bullet_lines(count).len() as u16 + 2;
    // The acknowledgement must always be fully visible; size its row to it.
    let acknowledged = app.write_session.acknowledged();
    let acknowledgment = wrapped_lines(
        &format!(
            "{} {}",
            if acknowledged { "■" } else { "□" },
            t.text(Message::ConfirmAck)
        ),
        Style::default().fg(if acknowledged { ACCENT } else { Color::White }),
        text_width,
    );
    let acknowledgment_height = acknowledgment.len() as u16 + 2;
    // Full layout: target (4), changes (>= 5), consequences, acknowledgement,
    // buttons (3) and four gaps.
    let full_height = 4 + 5 + bullet_height(4) + acknowledgment_height + 3 + 4;
    let compact = inner.height < full_height;

    frame.render_widget(Clear, modal);
    let modal_title = format!(
        " {} · {} ",
        t.text(Message::ConfirmTitle),
        t.heading(Message::ConfirmBadge)
    );
    frame.render_widget(
        Block::default()
            .title(modal_title)
            .title_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Yellow))
            .style(Style::default().bg(PANEL)),
        modal,
    );
    let rows = if compact {
        Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(acknowledgment_height),
            Constraint::Length(3),
        ])
        .spacing(1)
        .split(inner)
    } else {
        Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(5),
            Constraint::Length(bullet_height(4)),
            Constraint::Length(acknowledgment_height),
            Constraint::Length(3),
        ])
        .spacing(1)
        .split(inner)
    };
    let physical_title = format!(" {} ", t.text(Message::ConfirmPhysicalTarget));
    frame.render_widget(
        Paragraph::new(target)
            .wrap(Wrap { trim: true })
            .block(panel_block(&physical_title)),
        rows[0],
    );
    let consequences_title = format!(" {} ", t.text(Message::ConfirmConsequences));
    if compact {
        // Narrow terminals show the two consequences that matter most; the
        // full list appears on a taller screen.
        frame.render_widget(
            Paragraph::new(bullet_lines(2)).block(panel_block(&consequences_title)),
            rows[1],
        );
    } else {
        let changes_title = format!(" {} ", t.text(Message::ConfirmChanges));
        frame.render_widget(
            List::new(changes).block(panel_block(&changes_title)),
            rows[1],
        );
        frame.render_widget(
            Paragraph::new(bullet_lines(4)).block(panel_block(&consequences_title)),
            rows[2],
        );
    }
    let acknowledgment_row = if compact { rows[2] } else { rows[3] };
    let actions_row = if compact { rows[3] } else { rows[4] };
    frame.render_widget(
        Paragraph::new(acknowledgment).block(panel_block(" Space/click to acknowledge ")),
        acknowledgment_row,
    );
    let actions = Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .spacing(1)
        .split(actions_row);
    render_button(
        frame,
        actions[0],
        &format!("←  {}", t.text(Message::ActionCancel)),
        false,
    );
    let confirm_ready = app.write_session.can_confirm();
    if confirm_ready {
        render_danger_button(
            frame,
            actions[1],
            &format!("!  {}", t.text(Message::ConfirmSubmit)),
        );
    } else {
        render_disabled_button(
            frame,
            actions[1],
            &format!("□  {}", t.text(Message::ConfirmAcknowledgeFirst)),
        );
    }
    app.hit_regions.confirm_acknowledge = Some(acknowledgment_row);
    app.hit_regions.confirm_cancel = Some(actions[0]);
    app.hit_regions.confirm_write = confirm_ready.then_some(actions[1]);
}

fn draw_workspace(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    if area.width >= 72 {
        let workspace =
            Layout::horizontal([Constraint::Percentage(46), Constraint::Percentage(54)])
                .spacing(1)
                .split(area);
        draw_source(frame, app, workspace[0]);
        draw_targets(frame, app, workspace[1]);
    } else {
        let workspace = Layout::vertical([Constraint::Percentage(48), Constraint::Percentage(52)])
            .spacing(1)
            .split(area);
        draw_source(frame, app, workspace[0]);
        draw_targets(frame, app, workspace[1]);
    }
}

fn draw_header(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let header_rows = Layout::vertical([Constraint::Length(3), Constraint::Length(1)])
        .spacing(1)
        .split(area);
    let area = header_rows[0];
    let wide = area.width >= 82;
    let t = app.t();
    let action_count = if app.image.is_some() { 4 } else { 3 };
    // Every button must at least fit its compact caption in this language
    // (glyph, space, text, borders and a column of padding).
    let caption = [
        Message::ActionDownloadsCompact,
        Message::ActionCatalogCloseCompact,
        Message::ActionDiscoverCompact,
        Message::ActionSetupOptionsCompact,
        Message::ActionHideOptionsCompact,
        Message::ActionRefresh,
    ]
    .iter()
    .map(|message| display_width(t.text(*message)))
    .max()
    .unwrap_or(0) as u16
        + 5;
    let needed = caption * action_count as u16 + (action_count as u16 - 1);
    // With room to spare the buttons grow to show the full wording, as long
    // as the brand lockup keeps its width.
    let full_caption = [
        Message::ActionDownloads,
        Message::ActionCatalogClose,
        Message::ActionDiscover,
        Message::ActionSetupOptions,
        Message::ActionHideOptions,
        Message::ActionRefreshDrives,
    ]
    .iter()
    .map(|message| display_width(t.text(*message)))
    .max()
    .unwrap_or(0) as u16
        + 5;
    // The lockup needs its title line and its subtitle line.
    let brand_need = (display_width(t.text(Message::HeaderSubtitleCreate)) + 6).max(
        4 + display_width(&format!("  BOOTABLE v{}", env!("CARGO_PKG_VERSION")))
            + display_width(&format!("  ·  {}", t.text(Message::HeaderTitleCreate))),
    ) as u16;
    let preferred = (full_caption * action_count as u16 + (action_count as u16 - 1))
        .min(area.width.saturating_sub(7 + brand_need));
    let action_width = if wide {
        if action_count == 4 { 64 } else { 48 }.max(preferred)
    } else {
        area.width.saturating_sub(13)
    }
    .max(needed)
    .min(area.width.saturating_sub(if wide { 13 + 24 } else { 13 }));
    let columns = Layout::horizontal([
        Constraint::Min(if wide { 24 } else { 12 }),
        Constraint::Length(action_width + if wide { 6 } else { 0 }),
    ])
    .spacing(1)
    .split(area);
    let action_columns = Layout::horizontal([Constraint::Min(0), Constraint::Length(5)])
        .spacing(1)
        .split(columns[1]);
    render_button(frame, action_columns[1], "?", app.help_open);
    app.hit_regions.guide = Some(action_columns[1]);
    frame.render_widget(
        Paragraph::new(brand_lockup(
            wide,
            t.text(Message::HeaderTitleCreate),
            t.text(Message::HeaderSubtitleCreate),
            t.text(Message::HeaderTagline),
            usize::from(columns[0].width),
        ))
        .style(Style::default().bg(BG)),
        columns[0],
    );
    let actions = Layout::horizontal(vec![
        Constraint::Ratio(1, action_count as u32);
        action_count
    ])
    .spacing(1)
    .split(action_columns[0]);
    render_button(
        frame,
        actions[0],
        &glyph_label(
            actions[0],
            !wide,
            "⇩",
            t.text(Message::ActionDownloads),
            t.text(Message::ActionDownloadsCompact),
        ),
        app.downloads_open,
    );
    render_button(
        frame,
        actions[1],
        &if app.catalog_open {
            glyph_label(
                actions[1],
                !wide,
                "×",
                t.text(Message::ActionCatalogClose),
                t.text(Message::ActionCatalogCloseCompact),
            )
        } else {
            glyph_label(
                actions[1],
                !wide,
                "⌄",
                t.text(Message::ActionDiscover),
                t.text(Message::ActionDiscoverCompact),
            )
        },
        app.workspace_focus == WorkspaceFocus::Discover,
    );
    if app.image.is_some() {
        render_button(
            frame,
            actions[2],
            &if app.advanced {
                glyph_label(
                    actions[2],
                    !wide,
                    "⚙",
                    t.text(Message::ActionHideOptions),
                    t.text(Message::ActionHideOptionsCompact),
                )
            } else {
                glyph_label(
                    actions[2],
                    !wide,
                    "⚙",
                    t.text(Message::ActionSetupOptions),
                    t.text(Message::ActionSetupOptionsCompact),
                )
            },
            false,
        );
        app.hit_regions.advanced = Some(actions[2]);
    } else {
        app.hit_regions.advanced = None;
    }
    let refresh_index = action_count - 1;
    render_button(
        frame,
        actions[refresh_index],
        &glyph_label(
            actions[refresh_index],
            !wide,
            "↻",
            t.text(Message::ActionRefreshDrives),
            t.text(Message::ActionRefresh),
        ),
        app.workspace_focus == WorkspaceFocus::Refresh,
    );
    app.hit_regions.downloads = Some(actions[0]);
    app.hit_regions.discover = Some(actions[1]);
    app.hit_regions.refresh = Some(actions[refresh_index]);
    draw_workspace_steps(frame, app, header_rows[1]);
}

fn draw_workspace_steps(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let progress = workspace_progress(
        app.image.as_ref(),
        app.selected.and_then(|index| app.devices.get(index)),
    );
    let titles = WorkspaceProgress::step_titles(app.locale);
    let line = Line::from(vec![
        step_span(format!("1 {}", titles[0]), progress.source),
        Span::styled("  ─────  ", Style::default().fg(BORDER)),
        step_span(format!("2 {}", titles[1]), progress.target),
        Span::styled("  ─────  ", Style::default().fg(BORDER)),
        step_span(format!("3 {}", titles[2]), progress.review),
    ]);
    let room = usize::from(area.width).saturating_sub(line.width() + 1);
    let taken = draw_language_hint(frame, app, area, room);
    let steps_area = Rect::new(
        area.x,
        area.y,
        area.width.saturating_sub(taken),
        area.height,
    );
    frame.render_widget(
        Paragraph::new(line).alignment(Alignment::Center),
        steps_area,
    );
}

fn step_span(label: String, state: WorkspaceStepState) -> Span<'static> {
    let (marker, style) = match state {
        WorkspaceStepState::Complete => (
            "✓",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        WorkspaceStepState::Active => (
            "›",
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        WorkspaceStepState::Blocked => ("·", Style::default().fg(MUTED)),
    };
    Span::styled(format!(" {marker} {label} "), style)
}

/// The brand lockup. `tagline` is shown after the context only when `room`
/// terminal columns leave space for it (the same rule the desktop header uses).
fn brand_lockup<'a>(
    wide: bool,
    context: &'a str,
    subtitle: &'a str,
    tagline: &'a str,
    room: usize,
) -> Vec<Line<'a>> {
    if !wide {
        return vec![Line::from(vec![
            Span::styled(
                format!(" USB♨  BOOTABLE v{} ", env!("CARGO_PKG_VERSION")),
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {context}"), Style::default().fg(Color::White)),
        ])];
    }
    let brand = format!("  BOOTABLE v{}", env!("CARGO_PKG_VERSION"));
    let context = format!("  ·  {context}");
    let tagline = format!("  ·  {tagline}");
    let used = 4 + display_width(&brand) + display_width(&context);
    let show_tagline = used + display_width(&tagline) <= room;
    vec![
        Line::from(vec![
            Span::styled(
                "┌┬┬┐",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                brand,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(context, Style::default().fg(Color::White)),
            Span::styled(
                if show_tagline { tagline } else { String::new() },
                Style::default().fg(MUTED),
            ),
        ]),
        Line::from(vec![
            Span::styled("╰♨─╯", Style::default().fg(ACCENT)),
            Span::styled(format!("  {subtitle}"), Style::default().fg(MUTED)),
        ]),
    ]
}

fn draw_source(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let t = app.t();
    let source = app
        .image
        .as_ref()
        .map(|image| {
            format!(
                "{}\n{} • {}  ·  ✓ {}",
                image.path.display(),
                image.kind,
                format_bytes(image.size),
                t.text(Message::SourceInspected)
            )
        })
        .unwrap_or_else(|| {
            format!(
                "{}\n{}",
                t.text(Message::SourceFormats),
                t.text(Message::SourceHint)
            )
        });
    let source_title = panel_heading(t, 1, Message::SourceTitle);
    let source_block =
        focused_panel_block(&source_title, app.workspace_focus == WorkspaceFocus::Source);
    let source_inner = source_block.inner(area);
    frame.render_widget(source_block, area);
    let recents = app.preferences.recent_images();
    let recent_lines = if source_inner.height >= 7 {
        (recents.len().max(1) + 1).min(usize::from(source_inner.height).saturating_sub(4))
    } else {
        0
    } as u16;
    let source_rows = Layout::vertical([Constraint::Min(3), Constraint::Length(recent_lines)])
        .split(source_inner);
    draw_recent_images(frame, app, &recents, source_rows[1]);
    // The button column grows to fit the longest caption in this language.
    let button_columns = [
        Message::ActionBrowse,
        Message::ActionChange,
        Message::ActionInspecting,
    ]
    .iter()
    .map(|message| display_width(t.text(*message)) + 5)
    .max()
    .unwrap_or(14)
    .clamp(14, usize::from(source_rows[0].width / 2).max(14)) as u16;
    // Narrow panels stack the button under the text instead of squeezing the
    // text into a few columns.
    let stacked = source_rows[0].width < 50 && source_rows[0].height >= 6;
    let source_columns = if stacked {
        let stack =
            Layout::vertical([Constraint::Min(2), Constraint::Length(3)]).split(source_rows[0]);
        let button = Layout::horizontal([Constraint::Min(0), Constraint::Length(button_columns)])
            .split(stack[1]);
        [stack[0], button[1]]
    } else {
        let columns = Layout::horizontal([Constraint::Min(16), Constraint::Length(button_columns)])
            .spacing(1)
            .split(source_rows[0]);
        [columns[0], columns[1]]
    };
    frame.render_widget(
        Paragraph::new(source)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: true }),
        source_columns[0],
    );
    let button_area = centered_button_area(source_columns[1]);
    if app.image_loading {
        render_disabled_button(frame, button_area, t.text(Message::ActionInspecting));
        app.hit_regions.open_image = None;
    } else {
        render_button(
            frame,
            button_area,
            &format!(
                "▣  {}",
                t.text(if app.image.is_some() {
                    Message::ActionChange
                } else {
                    Message::ActionBrowse
                })
            ),
            true,
        );
        app.hit_regions.open_image = Some(button_area);
    }
}

fn draw_recent_images(
    frame: &mut ratatui::Frame<'_>,
    app: &mut App,
    recents: &[bootable_core::RecentImage],
    area: Rect,
) {
    app.hit_regions.recent_rows.clear();
    if area.height == 0 {
        return;
    }
    if recents.is_empty() {
        frame.render_widget(
            Paragraph::new(truncate_end(
                app.t().text(Message::SourceRecentEmpty),
                usize::from(area.width),
            ))
            .style(Style::default().fg(MUTED)),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    }
    frame.render_widget(
        Paragraph::new(format!(
            "{} · 1-4",
            app.t().heading(Message::SourceRecentTitle)
        ))
        .style(Style::default().fg(MUTED)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    for (index, recent) in recents
        .iter()
        .enumerate()
        .take(usize::from(area.height) - 1)
    {
        let row = Rect::new(area.x, area.y + 1 + index as u16, area.width, 1);
        let current = app
            .image
            .as_ref()
            .is_some_and(|image| image.path == recent.path);
        let size = if current {
            app.t().text(Message::SourceRecentInUse).to_string()
        } else {
            format_bytes(recent.size)
        };
        let name_width = usize::from(row.width).saturating_sub(display_width(&size) + 5);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{} ", index + 1), Style::default().fg(ACCENT)),
                Span::styled(
                    pad_display(
                        &truncate_middle(&recent.file_name(), name_width),
                        name_width,
                    ),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("  {size}"),
                    Style::default().fg(if current { ACCENT } else { MUTED }),
                ),
            ])),
            row,
        );
        app.hit_regions.recent_rows.push((row, index));
    }
}

fn draw_download_manager(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let t = app.t();
    let locale = app.locale;
    let rows = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(4),
        Constraint::Length(3),
    ])
    .spacing(1)
    .split(area);
    let items = if app.download_session.jobs().is_empty() {
        vec![ListItem::new(t.text(Message::DownloadsEmpty)).style(Style::default().fg(MUTED))]
    } else {
        let status_width = app
            .download_session
            .jobs()
            .iter()
            .map(|job| display_width(job.status.label_in(locale)))
            .max()
            .unwrap_or(0)
            .max(11);
        app.download_session
            .jobs()
            .iter()
            .map(|job| {
                let progress = job
                    .progress_ratio()
                    .map(|ratio| format!(" · {:>5.1}%", ratio * 100.))
                    .unwrap_or_default();
                ListItem::new(format!(
                    "{} {} {}{}",
                    pad_display(job.status.label_in(locale), status_width),
                    pad_display(&truncate_middle(&job.label, 28), 28),
                    job.destination.display(),
                    progress
                ))
                .style(Style::default().fg(match job.status {
                    DownloadStatus::Completed => ACCENT,
                    DownloadStatus::Failed | DownloadStatus::Cancelled => Color::LightRed,
                    DownloadStatus::Interrupted | DownloadStatus::Paused => Color::Yellow,
                    DownloadStatus::Queued | DownloadStatus::Running => Color::White,
                }))
            })
            .collect::<Vec<_>>()
    };
    let mut state = ListState::default()
        .with_selected((!app.download_session.jobs().is_empty()).then_some(app.download_selected));
    let downloads_title = format!(
        " {} · {} ",
        t.text(Message::ActionDownloads),
        t.text(Message::DownloadsSubtitle)
    );
    frame.render_stateful_widget(
        List::new(items)
            .block(
                panel_block(&downloads_title)
                    .title_bottom(Line::from(" ↑/↓ select · m closes ").right_aligned()),
            )
            .highlight_symbol("› ")
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        rows[0],
        &mut state,
    );
    let details = app
        .download_session
        .jobs()
        .get(app.download_selected)
        .map_or_else(
            || t.text(Message::DownloadsInterruptedNote).to_string(),
            |job| {
                format!(
                    "{} · {}\n{}",
                    job.kind.label_in(locale),
                    job.error.as_deref().unwrap_or(&job.message),
                    job.destination.display()
                )
            },
        );
    let selected_download_title = format!(" {} ", t.text(Message::DownloadsSelected));
    frame.render_widget(
        Paragraph::new(details)
            .style(Style::default().fg(MUTED))
            .wrap(Wrap { trim: true })
            .block(panel_block(&selected_download_title)),
        rows[1],
    );
    let actions = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .spacing(1)
    .split(rows[2]);
    let selected = app.download_session.jobs().get(app.download_selected);
    let can_retry = selected.is_some_and(|job| job.status.can_retry());
    let can_use = selected.is_some_and(|job| job.status == DownloadStatus::Completed);
    let can_remove = selected
        .is_some_and(|job| !matches!(job.status, DownloadStatus::Running | DownloadStatus::Paused));
    let retry_label = format!("↻  {}", t.text(Message::DownloadsActionRetryResume));
    let use_label = format!("✓  {}", t.text(Message::DownloadsActionUseImage));
    let remove_label = format!("×  {}", t.text(Message::DownloadsActionRemove));
    if can_retry {
        render_button(frame, actions[0], &retry_label, true);
    } else {
        render_disabled_button(frame, actions[0], &retry_label);
    }
    if can_use {
        render_button(frame, actions[1], &use_label, true);
    } else {
        render_disabled_button(frame, actions[1], &use_label);
    }
    if can_remove {
        render_button(frame, actions[2], &remove_label, false);
    } else {
        render_disabled_button(frame, actions[2], &remove_label);
    }
    app.hit_regions.download_rows = catalog_row_regions(rows[0], app.download_session.jobs().len());
    app.hit_regions.download_retry = can_retry.then_some(actions[0]);
    app.hit_regions.download_use = can_use.then_some(actions[1]);
    app.hit_regions.download_remove = can_remove.then_some(actions[2]);
}

/// Terminal columns `value` occupies. East Asian glyphs take two, so
/// `chars().count()` is wrong for them.
fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

/// The first of `variants` (longest first) that fits in `columns` terminal
/// columns, else the last one.
fn fit_variant(columns: usize, variants: &[String]) -> String {
    variants
        .iter()
        .find(|variant| display_width(variant) <= columns)
        .or(variants.last())
        .cloned()
        .unwrap_or_default()
}

/// A button caption: `glyph` (drawn by the adapter, never part of the
/// translated text) plus the long wording, or the compact wording when the
/// layout is narrow or the long form would not fit inside `area`.
fn glyph_label(area: Rect, narrow: bool, glyph: &str, long: &str, compact: &str) -> String {
    let compact = format!("{glyph} {compact}");
    if narrow {
        return compact;
    }
    fit_variant(
        usize::from(area.width.saturating_sub(2)),
        &[format!("{glyph} {long}"), compact],
    )
}

/// `text` wrapped to `width` terminal columns, ready to render without
/// `Wrap`. The widget wrapper only breaks at spaces, which strands a whole
/// Japanese sentence on its own row and makes the row count unpredictable;
/// this one measures columns and may break between wide characters.
fn wrapped_lines(text: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    wrap_styled(&[(text, style)], width)
        .into_iter()
        .map(Line::from)
        .collect()
}

/// Pads `value` with spaces to `width` terminal columns (never truncates).
fn pad_display(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(value));
    format!("{value}{}", " ".repeat(padding))
}

/// The longest prefix of `value` that fits in `columns` terminal columns.
fn take_columns(value: impl Iterator<Item = char>, columns: usize) -> String {
    let mut used = 0;
    let mut taken = String::new();
    for character in value {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + width > columns {
            break;
        }
        used += width;
        taken.push(character);
    }
    taken
}

/// One list row: `prefix`, then `name` padded or ellipsized to whatever the
/// row has left, then `suffix`. The suffix (the action) is never clipped.
fn fit_row(room: usize, prefix: &str, name: &str, suffix: &str) -> String {
    let name_room = room
        .saturating_sub(display_width(prefix) + display_width(suffix))
        .max(6);
    format!(
        "{prefix}{}{suffix}",
        pad_display(&truncate_end(name, name_room), name_room)
    )
}

/// `value` with its first character upper-cased (a no-op for scripts without
/// case).
fn capitalize_first(value: &str) -> String {
    let mut characters = value.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect())
        .unwrap_or_default()
}

/// `value` cut to `limit` terminal columns with a trailing ellipsis.
fn truncate_end(value: &str, limit: usize) -> String {
    if display_width(value) <= limit {
        return value.into();
    }
    format!("{}…", take_columns(value.chars(), limit.saturating_sub(1)))
}

fn truncate_middle(value: &str, limit: usize) -> String {
    if display_width(value) <= limit {
        return value.into();
    }
    let side = limit.saturating_sub(1) / 2;
    let start = take_columns(value.chars(), side);
    let end = take_columns(value.chars().rev(), side)
        .chars()
        .rev()
        .collect::<String>();
    format!("{start}…{end}")
}

/// Greedy word wrap that measures terminal columns, preserves the style of
/// each segment, and may break between any two wide (CJK) characters because
/// those scripts have no word spacing.
fn wrap_styled(segments: &[(&str, Style)], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let cells = segments
        .iter()
        .flat_map(|(text, style)| text.chars().map(move |character| (character, *style)))
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    let mut start = 0;
    while start < cells.len() {
        if start > 0 {
            while start < cells.len() && cells[start].0 == ' ' {
                start += 1;
            }
            if start >= cells.len() {
                break;
            }
        }
        let (mut columns, mut end, mut last_break) = (0, start, None);
        while end < cells.len() {
            let glyph = UnicodeWidthChar::width(cells[end].0).unwrap_or(0);
            if columns + glyph > width && end > start {
                break;
            }
            columns += glyph;
            end += 1;
            if cells[end - 1].0 == ' ' || glyph == 2 {
                last_break = Some(end);
            }
        }
        let cut = if end >= cells.len() {
            end
        } else {
            last_break
                .filter(|position| *position > start)
                .unwrap_or(end)
        };
        let mut line = &cells[start..cut];
        while line.last().is_some_and(|(character, _)| *character == ' ') {
            line = &line[..line.len() - 1];
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut run = String::new();
        let mut run_style: Option<Style> = None;
        for (character, style) in line {
            if run_style.is_some_and(|current| current != *style) {
                spans.push(Span::styled(
                    std::mem::take(&mut run),
                    run_style.unwrap_or_default(),
                ));
            }
            run_style = Some(*style);
            run.push(*character);
        }
        if let Some(style) = run_style {
            spans.push(Span::styled(run, style));
        }
        lines.push(spans);
        start = cut;
    }
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

/// System default -> every available language -> System default.
fn next_language(current: Option<Locale>) -> Option<Locale> {
    let available = Locale::available();
    match current {
        None => available.first().copied(),
        Some(current) => available
            .iter()
            .position(|locale| *locale == current)
            .and_then(|index| available.get(index + 1).copied()),
    }
}

/// The language hint from most to least descriptive. `locale` is the
/// effective language, so the label itself is rendered in it.
fn language_hint_variants(language: Option<Locale>, locale: Locale) -> Vec<String> {
    let label = Message::LanguageLabel.text(locale);
    let name = locale.native_name();
    match language {
        Some(_) => vec![format!("{label}: {name}"), name.to_string()],
        None => vec![
            format!(
                "{label}: {} ({name})",
                Message::LanguageSystemDefault.text(locale)
            ),
            format!("{label}: {name}"),
            name.to_string(),
        ],
    }
}

/// Renders the clickable language hint right-aligned in a one-row `area` and
/// returns the columns it took (including one column of padding), choosing the
/// most descriptive variant that fits in `room` columns.
fn draw_language_hint(
    frame: &mut ratatui::Frame<'_>,
    app: &mut App,
    area: Rect,
    room: usize,
) -> u16 {
    let variants = language_hint_variants(app.preferences.language, app.locale);
    let hint = variants
        .iter()
        .find(|variant| display_width(variant) <= room)
        .or(variants.last())
        .cloned()
        .unwrap_or_default();
    let width = (display_width(&hint) as u16).min(area.width);
    let region = Rect::new(area.right().saturating_sub(width), area.y, width, 1);
    frame.render_widget(
        Paragraph::new(hint).style(
            Style::default()
                .fg(MUTED)
                .add_modifier(Modifier::UNDERLINED),
        ),
        region,
    );
    app.hit_regions.language = Some(region);
    width + 1
}

/// Heading of a numbered workspace panel: the step title, then the panel's
/// own call to action (`source.title` / `target.title`) when it has one.
fn panel_heading(t: Strings, step: usize, qualifier: Message) -> String {
    let title = WorkspaceProgress::step_titles(t.locale())[step - 1];
    format!(" {step}  {title} · {} ", t.text(qualifier))
}

/// Heading of the review panel, which has no qualifier.
fn plain_panel_heading(t: Strings, step: usize) -> String {
    let title = WorkspaceProgress::step_titles(t.locale())[step - 1];
    format!(" {step}  {title} ")
}

fn draw_catalog(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let t = app.t();
    frame.render_widget(Clear, area);
    let discover_title = format!(" {} ", t.text(Message::DiscoverTitle));
    let block = panel_block(&discover_title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let compact_tabs = inner.width < 86;
    let single_toolbar = inner.width >= 118;
    let toolbar_height = if single_toolbar {
        3
    } else if compact_tabs {
        11
    } else {
        7
    };
    let rows = Layout::vertical([
        Constraint::Length(toolbar_height),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .spacing(u16::from(inner.height >= 24))
    .split(inner);
    let (source_area, search_area) = if single_toolbar {
        let toolbar = Layout::horizontal([Constraint::Percentage(66), Constraint::Percentage(34)])
            .spacing(1)
            .split(rows[0]);
        (toolbar[0], toolbar[1])
    } else {
        let toolbar = Layout::vertical([
            Constraint::Length(if compact_tabs { 7 } else { 3 }),
            Constraint::Length(3),
        ])
        .spacing(1)
        .split(rows[0]);
        (toolbar[0], toolbar[1])
    };
    let sources = grid_areas(
        source_area,
        if compact_tabs && !single_toolbar {
            3
        } else {
            6
        },
        6,
    );
    render_button(
        frame,
        sources[0],
        &format!("1  {}", t.text(Message::DiscoverQuickAll)),
        app.discovery_session.quick_access() == QuickAccess::All
            && app.discovery_session.source() == DiscoverySource::DistroWatch,
    );
    render_button(
        frame,
        sources[1],
        "2  Arch",
        app.discovery_session.quick_access() == QuickAccess::Arch,
    );
    render_button(
        frame,
        sources[2],
        "3  Debian",
        app.discovery_session.quick_access() == QuickAccess::Debian,
    );
    render_button(
        frame,
        sources[3],
        "4  Omarchy",
        app.discovery_session.quick_access() == QuickAccess::Omarchy,
    );
    render_button(
        frame,
        sources[4],
        "5  Windows",
        app.discovery_session.quick_access() == QuickAccess::Windows,
    );
    render_button(
        frame,
        sources[5],
        "6  Raspberry Pi",
        app.discovery_session.source() == DiscoverySource::RaspberryPi,
    );
    app.hit_regions.source_distrowatch = Some(sources[0]);
    app.hit_regions.source_arch = Some(sources[1]);
    app.hit_regions.source_debian = Some(sources[2]);
    app.hit_regions.source_omarchy = Some(sources[3]);
    app.hit_regions.source_windows = Some(sources[4]);
    app.hit_regions.source_raspberry_pi = Some(sources[5]);

    let search_style = if app.catalog_searching {
        Style::default().fg(Color::White).bg(Color::Rgb(25, 52, 47))
    } else {
        Style::default().fg(MUTED)
    };
    let search_room = usize::from(search_area.width.saturating_sub(2));
    let search_value = if app.discovery_session.quick_access() == QuickAccess::Windows {
        truncate_end(t.text(Message::DiscoverWindowsHint), search_room)
    } else if app.catalog_query.is_empty() {
        truncate_end(
            &format!("/ {}", t.text(Message::DiscoverSearchPlaceholder)),
            search_room,
        )
    } else {
        format!(
            "{}{}",
            app.catalog_query,
            if app.catalog_searching { "▏" } else { "" }
        )
    };
    let search_title = format!(" {} ", t.text(Message::DiscoverSearchTitle));
    frame.render_widget(
        Paragraph::new(search_value)
            .style(search_style)
            .block(panel_block(&search_title)),
        search_area,
    );
    app.hit_regions.catalog_search = Some(search_area);

    if app.discovery_session.quick_access() == QuickAccess::Windows {
        draw_windows_catalog(frame, app, rows[1]);
    } else {
        match app.discovery_session.source() {
            DiscoverySource::DistroWatch => draw_distrowatch_catalog(frame, app, rows[1]),
            DiscoverySource::RaspberryPi => draw_pi_catalog(frame, app, rows[1]),
        }
    }

    let actions = Layout::horizontal([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)])
        .spacing(1)
        .split(rows[2]);
    let active_state = match app.discovery_session.source() {
        DiscoverySource::RaspberryPi => app.discovery_session.state(CatalogFacet::RaspberryPi),
        DiscoverySource::DistroWatch if !app.catalog_query.is_empty() => {
            app.discovery_session.state(CatalogFacet::Directory)
        }
        DiscoverySource::DistroWatch => match app.discovery_session.quick_access() {
            QuickAccess::Arch => app.discovery_session.state(CatalogFacet::Arch),
            QuickAccess::Debian => app.discovery_session.state(CatalogFacet::Debian),
            _ => app.discovery_session.state(CatalogFacet::Popular),
        },
    };
    let refresh_label = format!(
        "↻  {}",
        t.text(
            if active_state.is_failed()
                || app
                    .discovery_session
                    .state(CatalogFacet::Details)
                    .is_failed()
            {
                Message::ActionRetry
            } else {
                Message::ActionRefresh
            }
        )
    );
    render_button(frame, actions[0], &refresh_label, false);
    let open_page_fallback = app.discovery_session.source() == DiscoverySource::DistroWatch
        && app.discovery_session.quick_access() != QuickAccess::Windows
        && app.catalog_releases.is_empty()
        && !app.distributions.is_empty()
        && !app
            .discovery_session
            .state(CatalogFacet::Details)
            .is_loading();
    let can_download = if app.discovery_session.quick_access() == QuickAccess::Windows {
        true
    } else {
        match app.discovery_session.source() {
            DiscoverySource::DistroWatch => !app.catalog_releases.is_empty(),
            DiscoverySource::RaspberryPi => app.pi_catalog.is_some(),
        }
    };
    render_button(
        frame,
        actions[1],
        &if app.discovery_session.quick_access() == QuickAccess::Windows {
            let windows_ready = app.image.as_ref().is_some_and(|image| {
                matches!(
                    image.kind,
                    bootable_core::ImageKind::WindowsInstaller { .. }
                )
            });
            format!(
                "▣  {}",
                t.text(if windows_ready {
                    Message::OptionsWindowsReplaceIso
                } else {
                    Message::OptionsWindowsChooseIso
                })
            )
        } else if app.discovery_session.source() == DiscoverySource::RaspberryPi {
            format!("⇩  {}", t.text(Message::PiDownloadUse))
        } else if open_page_fallback {
            format!("↗  {}  [b]", t.text(Message::DiscoverDetailOpenPage))
        } else {
            format!("⇩  {}", t.text(Message::DiscoverDetailDownloadUse))
        },
        can_download || open_page_fallback,
    );
    app.hit_regions.catalog_retry = Some(actions[0]);
    app.hit_regions.catalog_close = None;
    app.hit_regions.catalog_download = Some(actions[1]);
}

fn draw_windows_catalog(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    app.hit_regions.distribution_rows.clear();
    app.hit_regions.release_rows.clear();
    app.hit_regions.pi_device_rows.clear();
    app.hit_regions.pi_image_rows.clear();
    let t = app.t();
    let windows_image = app.image.as_ref().is_some_and(|image| {
        matches!(
            image.kind,
            bootable_core::ImageKind::WindowsInstaller { .. }
        )
    });
    let columns = windows_option_columns(area.width);
    let option_rows = 12_usize.div_ceil(columns);
    let option_height = (option_rows * 3 + option_rows.saturating_sub(1)) as u16;
    let header_height = if area.height >= option_height.saturating_add(13) {
        7
    } else {
        5
    };
    let rows = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Length(option_height),
        Constraint::Min(3),
    ])
    .spacing(u16::from(area.height >= option_height.saturating_add(10)))
    .split(area);
    let header_lines = if header_height >= 7 {
        vec![
            Line::styled(
                "Windows installer media · Rufus-inspired workflow",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                "GPT or MBR + UEFI FAT32 · split WIM above 4 GiB · verification · removable-drive safety gates",
                Style::default().fg(MUTED),
            ),
            Line::styled(
                "MD5 / SHA-1 / SHA-256 / SHA-512 · 1, 2, or 4-pass bad-block test · reviewed erase phrase",
                Style::default().fg(MUTED),
            ),
            Line::styled(
                if windows_image {
                    "Windows ISO recognized · each setup customization remains independently selectable"
                } else {
                    "Press o, Enter, or click Choose Windows ISO to unlock setup customizations"
                },
                Style::default().fg(if windows_image { ACCENT } else { MUTED }),
            ),
            Line::styled(
                "Rufus 4.15 inventory below: ✓ available now · ○ not implemented",
                Style::default().fg(Color::Yellow),
            ),
        ]
    } else {
        vec![
            Line::styled(
                "Windows media · UEFI FAT32 · split WIM · verified writes",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::styled(
                t.text(if windows_image {
                    Message::OptionsWindowsInstallerReady
                } else {
                    Message::OptionsWindowsInstallerLocked
                }),
                Style::default().fg(if windows_image { ACCENT } else { MUTED }),
            ),
            Line::styled(
                "✓ available · ○ unavailable",
                Style::default().fg(Color::Yellow),
            ),
        ]
    };
    frame.render_widget(
        Paragraph::new(header_lines)
            .wrap(Wrap { trim: true })
            .block(panel_block(" Windows media features ")),
        rows[0],
    );
    let options = grid_areas(rows[1], columns, 12);
    let named_account = app
        .options
        .windows
        .local_account
        .clone()
        .or_else(bootable_core::suggested_account_name)
        .unwrap_or_else(|| "User".into());
    let option_cells = [
        (
            Message::OptionsWindowsBypassHardwareLabel,
            Message::OptionsWindowsBypassHardwareShort,
            app.options.windows.bypass_hardware_requirements,
        ),
        (
            Message::OptionsWindowsOfflineAccountLabel,
            Message::OptionsWindowsOfflineAccountShort,
            app.options.windows.allow_offline_account,
        ),
        (
            Message::OptionsWindowsPrivacyLabel,
            Message::OptionsWindowsPrivacyShort,
            app.options.windows.minimize_data_collection,
        ),
        (
            Message::OptionsWindowsBitlockerLabel,
            Message::OptionsWindowsBitlockerShort,
            app.options.windows.disable_bitlocker,
        ),
        (
            Message::OptionsWindowsNamedAccountLabel,
            Message::OptionsWindowsNamedAccountShort,
            app.options.windows.local_account.is_some(),
        ),
        (
            Message::OptionsWindowsHostRegionLabel,
            Message::OptionsWindowsHostRegionShort,
            app.options.windows.regional.is_some(),
        ),
        (
            Message::OptionsWindowsQolLabel,
            Message::OptionsWindowsQolShort,
            app.options.windows.quality_of_life,
        ),
        (
            Message::OptionsWindowsCa2023Label,
            Message::OptionsWindowsCa2023Short,
            app.options.windows.use_windows_ca_2023,
        ),
        (
            Message::OptionsWindowsSkusipolicyLabel,
            Message::OptionsWindowsSkusipolicyShort,
            app.options.windows.apply_skusi_policy,
        ),
        (
            Message::OptionsWindowsSmodeLabel,
            Message::OptionsWindowsSmodeShort,
            app.options.windows.force_s_mode,
        ),
    ];
    for (cell, (label, short, selected)) in option_cells.into_iter().enumerate() {
        // The named-account label carries a `{name}` placeholder; when narrow
        // the placeholder-free short caption is used instead.
        let label = if label == Message::OptionsWindowsNamedAccountLabel {
            t.format(label, &[("name", &named_account)])
        } else {
            t.text(label).to_string()
        };
        render_option_checkbox(frame, options[cell], &label, t.text(short), selected);
    }
    render_button(
        frame,
        options[10],
        &t.format(
            Message::OptionsWindowsSchemeValue,
            &[("scheme", &app.options.windows_partition_scheme)],
        ),
        true,
    );
    if windows_image {
        render_button(
            frame,
            options[11],
            &boot_firmware_label(t, app.options.windows_boot_firmware, options[11].width),
            true,
        );
    }
    if windows_image {
        app.hit_regions.windows_options = Some(options[0]);
        app.hit_regions.windows_offline = Some(options[1]);
        app.hit_regions.windows_privacy = Some(options[2]);
        app.hit_regions.windows_bitlocker = Some(options[3]);
        app.hit_regions.windows_named_account = Some(options[4]);
        app.hit_regions.windows_regional = Some(options[5]);
        app.hit_regions.windows_qol = Some(options[6]);
        app.hit_regions.windows_ca_2023 = Some(options[7]);
        app.hit_regions.windows_skusi_policy = Some(options[8]);
        app.hit_regions.windows_s_mode = Some(options[9]);
        app.hit_regions.windows_partition_scheme = Some(options[10]);
        app.hit_regions.windows_boot_firmware = Some(options[11]);
    } else {
        app.hit_regions.windows_options = None;
        app.hit_regions.windows_offline = None;
        app.hit_regions.windows_privacy = None;
        app.hit_regions.windows_bitlocker = None;
        app.hit_regions.windows_named_account = None;
        app.hit_regions.windows_regional = None;
        app.hit_regions.windows_qol = None;
        app.hit_regions.windows_ca_2023 = None;
        app.hit_regions.windows_skusi_policy = None;
        app.hit_regions.windows_s_mode = None;
        app.hit_regions.windows_partition_scheme = None;
        app.hit_regions.windows_boot_firmware = None;
    }
    let mut coverage = vec![
        Line::styled(
            "✓ Standard install · GPT/UEFI/FAT32 · split WIM · requirements · account · region · privacy · BitLocker",
            Style::default().fg(ACCENT),
        ),
        Line::styled(
            "✓ QoL · CA 2023 · SkuSiPolicy · S Mode · checksums · bad blocks · verified write · safety gates",
            Style::default().fg(ACCENT),
        ),
    ];
    if windows_image {
        coverage.push(Line::styled(
            format!("f · {}", t.text(Message::OptionsWindowsBootFirmwareHint)),
            Style::default().fg(Color::Yellow),
        ));
    }
    coverage.push(Line::styled(
        "○ Windows To Go/internal-disk isolation · NTFS/UEFI:NTFS · silent install",
        Style::default().fg(Color::Yellow),
    ));
    coverage.push(Line::styled(
        t.text(Message::OptionsWindowsUnavailableNote),
        Style::default().fg(MUTED),
    ));
    frame.render_widget(
        Paragraph::new(coverage)
            .wrap(Wrap { trim: true })
            .block(panel_block(" Complete Rufus 4.15 Windows coverage ")),
        rows[2],
    );
}

fn draw_distrowatch_catalog(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let t = app.t();
    let locale = app.locale;
    app.hit_regions.pi_device_rows.clear();
    app.hit_regions.pi_image_rows.clear();
    let columns = if area.width >= 72 {
        Layout::horizontal([Constraint::Percentage(36), Constraint::Percentage(64)])
            .spacing(1)
            .split(area)
    } else {
        Layout::vertical([Constraint::Percentage(42), Constraint::Percentage(58)])
            .spacing(1)
            .split(area)
    };
    let matching_indices = app
        .filtered_distribution_indices()
        .into_iter()
        .take(app.catalog_visible)
        .collect::<Vec<_>>();
    let distributions = if matching_indices.is_empty() {
        let message = if !app.catalog_query.is_empty()
            && matches!(
                app.discovery_session.state(CatalogFacet::Directory),
                CatalogState::Ready { .. } | CatalogState::Empty
            ) {
            catalog_search_summary(&app.catalog_query, 0)
        } else if !app.catalog_query.is_empty() {
            app.discovery_session
                .state(CatalogFacet::Directory)
                .short_label_in(locale, t.text(Message::CatalogSubjectSearchCatalog))
        } else {
            current_distribution_state(app)
                .short_label_in(locale, t.text(Message::CatalogSubjectDistributions))
        };
        vec![ListItem::new(message).style(Style::default().fg(MUTED))]
    } else {
        matching_indices
            .iter()
            .filter_map(|index| {
                app.distributions
                    .get(*index)
                    .map(|distribution| (*index, distribution))
            })
            .map(|(index, distribution)| {
                let action = if app.catalog_selected == index {
                    t.text(Message::ActionSelected).to_string()
                } else {
                    format!("{} →", t.text(Message::ActionSelect))
                };
                // The name gives way so the action always stays visible.
                let room = usize::from(columns[0].width.saturating_sub(4));
                ListItem::new(if !app.catalog_query.is_empty() {
                    let rank = if distribution.rank == 0 {
                        "·".into()
                    } else {
                        distribution.rank.to_string()
                    };
                    let based = distribution
                        .based_on
                        .as_deref()
                        .unwrap_or(t.text(Message::DiscoverItemIndependent));
                    fit_row(
                        room,
                        &format!("{rank:>2}  "),
                        &distribution.name,
                        &format!(" {} {action}", pad_display(&truncate_end(based, 12), 12)),
                    )
                } else if distribution.rank == 0 {
                    fit_row(room, " ·  ", &distribution.name, &format!(" {action}"))
                } else {
                    let hits = t.format(
                        Message::DiscoverItemHitsPerDay,
                        &[("hits", &distribution.hits_per_day)],
                    );
                    fit_row(
                        room,
                        &format!("{:>2}  ", distribution.rank),
                        &distribution.name,
                        &format!(
                            " {}{hits}  {action}",
                            " ".repeat(9usize.saturating_sub(display_width(&hits)))
                        ),
                    )
                })
            })
            .collect::<Vec<_>>()
    };
    let releases = if app.catalog_releases.is_empty() {
        vec![
            ListItem::new(
                app.discovery_session
                    .state(CatalogFacet::Details)
                    .short_label_in(locale, t.text(Message::CatalogSubjectIsoReleases)),
            )
            .style(Style::default().fg(MUTED)),
        ]
    } else {
        app.catalog_releases
            .iter()
            .map(|release| {
                let integrity = release
                    .checksum_algorithm
                    .filter(|_| release.checksum.is_some() || release.checksum_url.is_some())
                    .map(|algorithm| format!("✓ {algorithm}"))
                    .unwrap_or_else(|| t.text(Message::DiscoverItemHttpsOnly).to_string());
                ListItem::new(format!(
                    "{}  {}  {}",
                    release.name,
                    release.size.map(format_bytes).unwrap_or_default(),
                    integrity
                ))
            })
            .collect::<Vec<_>>()
    };
    let selected_position = matching_indices
        .iter()
        .position(|index| *index == app.catalog_selected)
        .unwrap_or_default();
    let mut distribution_state = ListState::default()
        .with_selected((!matching_indices.is_empty()).then_some(selected_position));
    let mut release_state = ListState::default()
        .with_selected((!app.catalog_releases.is_empty()).then_some(app.release_selected));
    let distribution_title = format!(
        " {} ",
        if !app.catalog_query.is_empty() {
            t.text(Message::DiscoverSectionSearch)
        } else {
            match app.discovery_session.quick_access() {
                QuickAccess::Arch => t.text(Message::DiscoverSectionArch),
                QuickAccess::Debian => t.text(Message::DiscoverSectionDebian),
                QuickAccess::Omarchy => "Omarchy",
                _ => t.text(Message::DiscoverSectionPopular),
            }
        }
    );
    frame.render_stateful_widget(
        List::new(distributions)
            .block(panel_block(&distribution_title))
            .style(Style::default().fg(Color::White))
            .highlight_symbol("› ")
            .highlight_style(catalog_highlight(
                app.catalog_focus == CatalogFocus::Distributions,
            )),
        columns[0],
        &mut distribution_state,
    );
    let profile_height = if columns[1].height >= 12 { 7 } else { 4 };
    let right = Layout::vertical([Constraint::Length(profile_height), Constraint::Min(3)])
        .spacing(1)
        .split(columns[1]);
    let profile = if app.selected_details.is_none()
        && !matches!(
            app.discovery_session.state(CatalogFacet::Details),
            CatalogState::Idle
        ) {
        app.discovery_session
            .state(CatalogFacet::Details)
            .short_label_in(locale, t.text(Message::CatalogSubjectDistributionProfile))
    } else {
        app.selected_details.as_ref().map_or_else(
            || t.text(Message::DiscoverDetailEmpty).to_string(),
            |details| {
                let labeled = |label: Message, value: &dyn std::fmt::Display| {
                    t.format(
                        Message::CommonLabeled,
                        &[("label", &t.text(label)), ("value", value)],
                    )
                };
                let not_listed = t.text(Message::DiscoverDetailNotListed);
                format!(
                    "{}  ·  {}  ·  {}\n{}  ·  {}\n{}\n{}\n{}\n{}\n{}",
                    details.name,
                    details
                        .os_type
                        .as_deref()
                        .unwrap_or(t.text(Message::DiscoverDetailUnknownOs)),
                    details
                        .status
                        .as_deref()
                        .unwrap_or(t.text(Message::DiscoverDetailUnknownStatus)),
                    labeled(
                        Message::DiscoverDetailBasedOn,
                        &details
                            .based_on
                            .as_deref()
                            .unwrap_or(t.text(Message::DiscoverItemIndependent))
                    ),
                    labeled(
                        Message::DiscoverDetailOrigin,
                        &details
                            .origin
                            .as_deref()
                            .unwrap_or(t.text(Message::DiscoverDetailUnknownOrigin))
                    ),
                    labeled(
                        Message::DiscoverDetailArchitecture,
                        &compact_text_list(t, &details.architectures, 4)
                    ),
                    labeled(
                        Message::DiscoverDetailDesktop,
                        &compact_text_list(t, &details.desktops, 4)
                    ),
                    details
                        .description
                        .as_deref()
                        .unwrap_or(t.text(Message::DiscoverDetailNoDescription)),
                    labeled(
                        Message::DiscoverDetailLogo,
                        &details.logo_url.as_deref().unwrap_or(not_listed)
                    ),
                    labeled(
                        Message::DiscoverDetailScreenshot,
                        &details.screenshot_url.as_deref().unwrap_or(not_listed)
                    ),
                )
            },
        )
    };
    let profile_title = format!(
        " {} ",
        capitalize_first(t.text(Message::CatalogSubjectDistributionProfile))
    );
    draw_catalog_artwork_panel(frame, app, right[0], &profile_title, profile);
    let releases_title = format!(" {} ", t.text(Message::DiscoverDetailDirectIsos));
    frame.render_stateful_widget(
        List::new(releases)
            .block(panel_block(&releases_title))
            .style(Style::default().fg(Color::White))
            .highlight_symbol("› ")
            .highlight_style(catalog_highlight(
                app.catalog_focus == CatalogFocus::Releases,
            )),
        right[1],
        &mut release_state,
    );
    app.hit_regions.distribution_rows =
        catalog_row_regions_with_indices(columns[0], matching_indices);
    app.hit_regions.release_rows = catalog_row_regions(right[1], app.catalog_releases.len());
}

fn draw_pi_catalog(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let t = app.t();
    let locale = app.locale;
    app.hit_regions.distribution_rows.clear();
    app.hit_regions.release_rows.clear();
    let columns = if area.width >= 72 {
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
            .spacing(1)
            .split(area)
    } else {
        Layout::vertical([Constraint::Percentage(38), Constraint::Percentage(62)])
            .spacing(1)
            .split(area)
    };
    let devices = app.pi_catalog.as_ref().map_or_else(
        || {
            vec![
                ListItem::new(
                    app.discovery_session
                        .state(CatalogFacet::RaspberryPi)
                        .short_label_in(locale, t.text(Message::CatalogSubjectPiBoards)),
                )
                .style(Style::default().fg(MUTED)),
            ]
        },
        |catalog| {
            catalog
                .devices
                .iter()
                .map(|device| ListItem::new(device.name.clone()))
                .collect::<Vec<_>>()
        },
    );
    let visible_images = app
        .pi_catalog
        .as_ref()
        .map(|catalog| {
            app.compatible_pi_image_indices()
                .into_iter()
                .take(app.pi_visible)
                .filter_map(|index| {
                    catalog
                        .images
                        .get(index)
                        .cloned()
                        .map(|image| (index, image))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let image_position = visible_images
        .iter()
        .position(|(index, _)| *index == app.pi_image_selected)
        .unwrap_or_default();
    let image_items = if visible_images.is_empty() {
        let message = if app.pi_catalog.is_some() && !app.catalog_query.is_empty() {
            t.format(
                Message::PiEmptyQuery,
                &[("query", &app.catalog_query.trim())],
            )
        } else if app.pi_catalog.is_some() {
            t.text(Message::PiEmpty).to_string()
        } else {
            app.discovery_session
                .state(CatalogFacet::RaspberryPi)
                .short_label_in(locale, t.text(Message::CatalogSubjectPiImages))
        };
        vec![ListItem::new(message).style(Style::default().fg(MUTED))]
    } else {
        visible_images
            .iter()
            .map(|(_, image)| {
                ListItem::new(format!(
                    "{}  {}",
                    image.name,
                    image.download_size.map(format_bytes).unwrap_or_default()
                ))
            })
            .collect::<Vec<_>>()
    };
    let has_devices = app
        .pi_catalog
        .as_ref()
        .is_some_and(|catalog| !catalog.devices.is_empty());
    let mut device_state =
        ListState::default().with_selected(has_devices.then_some(app.pi_device_selected));
    let board_title = format!(" {} ", t.text(Message::PiBoardFilter));
    frame.render_stateful_widget(
        List::new(devices)
            .block(panel_block(&board_title))
            .style(Style::default().fg(Color::White))
            .highlight_symbol("› ")
            .highlight_style(catalog_highlight(
                app.catalog_focus == CatalogFocus::Distributions,
            )),
        columns[0],
        &mut device_state,
    );
    let details_height = if columns[1].height >= 11 { 6 } else { 4 };
    let right = Layout::vertical([Constraint::Length(details_height), Constraint::Min(3)])
        .spacing(1)
        .split(columns[1]);
    let selected = app
        .pi_catalog
        .as_ref()
        .and_then(|catalog| catalog.images.get(app.pi_image_selected));
    let details = selected.map_or_else(
        || t.text(Message::PiHint).to_string(),
        |image| {
            let sha = t.format(
                Message::CommonLabeled,
                &[
                    ("label", &"SHA-256"),
                    (
                        "value",
                        &if image.extracted_sha256.is_some() {
                            "✓"
                        } else {
                            t.text(Message::DiscoverDetailNotListed)
                        },
                    ),
                ],
            );
            format!(
                "{}\n{}\n{}  ·  {}  ·  {sha}",
                image.name,
                t.format(
                    Message::PiDetails,
                    &[
                        (
                            "download",
                            &image.download_size.map(format_bytes).unwrap_or_default()
                        ),
                        (
                            "expanded",
                            &image.extracted_size.map(format_bytes).unwrap_or_default()
                        ),
                        (
                            "date",
                            &image
                                .release_date
                                .as_deref()
                                .unwrap_or(t.text(Message::PiDateUnknown))
                        ),
                        (
                            "description",
                            &image
                                .description
                                .as_deref()
                                .unwrap_or(t.text(Message::DiscoverDetailNoDescription))
                        ),
                    ],
                ),
                image
                    .category
                    .as_deref()
                    .unwrap_or(t.text(Message::PiDefaultCategory)),
                image.archive_name,
            )
        },
    );
    let image_details_title = format!(" {} ", t.text(Message::PiTitle));
    draw_catalog_artwork_panel(frame, app, right[0], &image_details_title, details);
    let mut image_state =
        ListState::default().with_selected((!visible_images.is_empty()).then_some(image_position));
    let images_title = format!(" {} ", t.text(Message::PiCompatibleImages));
    frame.render_stateful_widget(
        List::new(image_items)
            .block(panel_block(&images_title))
            .style(Style::default().fg(Color::White))
            .highlight_symbol("› ")
            .highlight_style(catalog_highlight(
                app.catalog_focus == CatalogFocus::Releases,
            )),
        right[1],
        &mut image_state,
    );
    let device_count = app
        .pi_catalog
        .as_ref()
        .map(|catalog| catalog.devices.len())
        .unwrap_or_default();
    app.hit_regions.pi_device_rows = catalog_row_regions(columns[0], device_count);
    app.hit_regions.pi_image_rows =
        catalog_row_regions_with_indices(right[1], visible_images.iter().map(|(index, _)| *index));
}

fn draw_catalog_artwork_panel(
    frame: &mut ratatui::Frame<'_>,
    app: &mut App,
    area: Rect,
    title: &str,
    text: String,
) {
    let t = app.t();
    let block = panel_block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 34 || inner.height < 3 {
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(MUTED))
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }
    let columns = Layout::horizontal([
        Constraint::Length((inner.width / 3).clamp(10, 24)),
        Constraint::Min(20),
    ])
    .spacing(1)
    .split(inner);
    if let Some(protocol) = app.artwork_protocol.as_mut() {
        frame.render_stateful_widget(
            StatefulImage::new().resize(Resize::Fit(None)),
            columns[0],
            protocol,
        );
    } else {
        let artwork_status = app.artwork_error.as_deref().map_or_else(
            || {
                t.text(if app.artwork_key.is_some() {
                    Message::DiscoverArtworkLoading
                } else {
                    Message::DiscoverArtworkNone
                })
            },
            |_| t.text(Message::DiscoverArtworkUnavailable),
        );
        frame.render_widget(
            Paragraph::new(artwork_status)
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED))
                .wrap(Wrap { trim: true }),
            columns[0],
        );
    }
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(MUTED))
            .wrap(Wrap { trim: true }),
        columns[1],
    );
}

fn compact_text_list(t: Strings, values: &[String], limit: usize) -> String {
    if values.is_empty() {
        return t.text(Message::DiscoverDetailNotListed).into();
    }
    let mut value = values
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if values.len() > limit {
        value.push_str(&format!(" +{}", values.len() - limit));
    }
    value
}

fn current_distribution_state(app: &App) -> &CatalogState {
    match app.discovery_session.quick_access() {
        QuickAccess::Arch => app.discovery_session.state(CatalogFacet::Arch),
        QuickAccess::Debian => app.discovery_session.state(CatalogFacet::Debian),
        QuickAccess::All | QuickAccess::Omarchy | QuickAccess::Windows => {
            app.discovery_session.state(CatalogFacet::Popular)
        }
    }
}

fn catalog_row_regions_with_indices(
    area: Rect,
    indices: impl IntoIterator<Item = usize>,
) -> Vec<(Rect, usize)> {
    let inner = area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 1,
    });
    indices
        .into_iter()
        .enumerate()
        .filter_map(|(row, index)| {
            let y = inner.y.saturating_add(row as u16);
            (y < inner.bottom()).then_some((Rect::new(inner.x, y, inner.width, 1), index))
        })
        .collect()
}

fn catalog_highlight(focused: bool) -> Style {
    Style::default()
        .fg(if focused { Color::Black } else { ACCENT })
        .bg(if focused {
            ACCENT
        } else {
            Color::Rgb(21, 48, 47)
        })
        .add_modifier(Modifier::BOLD)
}

fn catalog_row_regions(area: Rect, count: usize) -> Vec<(Rect, usize)> {
    let inner = area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 1,
    });
    (0..count)
        .filter_map(|index| {
            let y = inner.y.saturating_add(index as u16);
            (y < inner.bottom()).then_some((Rect::new(inner.x, y, inner.width, 1), index))
        })
        .collect()
}

fn draw_advanced(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    app.hit_regions.windows_named_account = None;
    app.hit_regions.windows_regional = None;
    app.hit_regions.windows_qol = None;
    app.hit_regions.windows_ca_2023 = None;
    app.hit_regions.windows_skusi_policy = None;
    app.hit_regions.windows_s_mode = None;
    app.hit_regions.windows_partition_scheme = None;
    app.hit_regions.windows_boot_firmware = None;
    let t = app.t();
    let setup_title = format!(" {} ", t.text(Message::ActionSetupOptions));
    let block = focused_panel_block(&setup_title, app.workspace_focus == WorkspaceFocus::Setup);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let compact = area.width < 78;
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if compact { 7 } else { 3 }),
        Constraint::Min(if compact { 7 } else { 3 }),
    ])
    .split(inner);
    let selected = [
        app.options.windows.bypass_hardware_requirements,
        app.options.windows.allow_offline_account,
        app.options.windows.local_account.is_some(),
        app.options.windows.regional.is_some(),
        app.options.windows.minimize_data_collection,
        app.options.windows.disable_bitlocker,
        app.options.windows.quality_of_life,
        app.options.windows.use_windows_ca_2023,
        app.options.windows.apply_skusi_policy,
        app.options.windows.force_s_mode,
    ]
    .into_iter()
    .filter(|selected| *selected)
    .count();
    let windows_image = app.image.as_ref().is_some_and(|image| {
        matches!(
            image.kind,
            bootable_core::ImageKind::WindowsInstaller { .. }
        )
    });
    frame.render_widget(
        Paragraph::new(if windows_image {
            format!(
                "{}  ·  {}",
                t.text(Message::OptionsWindowsTitle),
                t.format(Message::OptionsSelectedCount, &[("count", &selected)])
            )
        } else {
            t.text(Message::OptionsLinuxTitle).to_string()
        })
        .style(Style::default().fg(MUTED)),
        rows[0],
    );
    let windows = grid_areas(rows[1], if compact { 2 } else { 4 }, 4);
    let tools = grid_areas(rows[2], if compact { 3 } else { 5 }, 5);
    if windows.len() < 4 || tools.len() < 5 {
        // Not enough room for the controls (a very short terminal): leave
        // the framed panel empty rather than index past the grid.
        return;
    }

    if windows_image {
        render_option_checkbox(
            frame,
            windows[0],
            t.text(Message::OptionsWindowsBypassHardwareLabel),
            t.text(Message::OptionsWindowsBypassHardwareShort),
            app.options.windows.bypass_hardware_requirements,
        );
        render_option_checkbox(
            frame,
            windows[1],
            t.text(Message::OptionsWindowsOfflineAccountLabel),
            t.text(Message::OptionsWindowsOfflineAccountShort),
            app.options.windows.allow_offline_account,
        );
        render_option_checkbox(
            frame,
            windows[2],
            t.text(Message::OptionsWindowsPrivacyLabel),
            t.text(Message::OptionsWindowsPrivacyShort),
            app.options.windows.minimize_data_collection,
        );
        render_option_checkbox(
            frame,
            windows[3],
            t.text(Message::OptionsWindowsBitlockerLabel),
            t.text(Message::OptionsWindowsBitlockerShort),
            app.options.windows.disable_bitlocker,
        );
        app.hit_regions.windows_options = Some(windows[0]);
        app.hit_regions.windows_offline = Some(windows[1]);
        app.hit_regions.windows_privacy = Some(windows[2]);
        app.hit_regions.windows_bitlocker = Some(windows[3]);
    } else {
        render_option_checkbox(
            frame,
            windows[0],
            t.text(Message::OptionsLinuxLayout),
            t.text(Message::OptionsLinuxLayoutShort),
            true,
        );
        render_option_checkbox(
            frame,
            windows[1],
            t.text(Message::OptionsLinuxBootRecordsShort),
            t.text(Message::OptionsLinuxBootRecordsShort),
            true,
        );
        render_option_checkbox(
            frame,
            windows[2],
            t.text(Message::OptionsLinuxVerify),
            t.text(Message::OptionsLinuxVerifyShort),
            true,
        );
        render_option_checkbox(
            frame,
            windows[3],
            t.text(Message::OptionsLinuxUnmountShort),
            t.text(Message::OptionsLinuxUnmountShort),
            true,
        );
        app.hit_regions.windows_options = None;
        app.hit_regions.windows_offline = None;
        app.hit_regions.windows_privacy = None;
        app.hit_regions.windows_bitlocker = None;
    }

    let bad_blocks = app.options.bad_block_check.label_in(app.locale);
    render_button(frame, tools[0], &format!("◌  {bad_blocks}"), false);
    render_button(
        frame,
        tools[1],
        &format!("#  {}", app.checksum_algorithm),
        false,
    );
    render_button(
        frame,
        tools[2],
        &format!("✓  {}", t.text(Message::OptionsToolsVerifyImage)),
        false,
    );
    render_button(
        frame,
        tools[3],
        &format!("▢  {}", t.text(Message::OptionsToolsImageFolder)),
        false,
    );
    render_button(
        frame,
        tools[4],
        &format!("⇩  {}", t.text(Message::OptionsToolsBackupDrive)),
        false,
    );

    app.hit_regions.bad_blocks = Some(tools[0]);
    app.hit_regions.checksum_algorithm = Some(tools[1]);
    app.hit_regions.checksum = Some(tools[2]);
    app.hit_regions.choose_folder = Some(tools[3]);
    app.hit_regions.backup = Some(tools[4]);
}

fn draw_targets(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let locale = app.locale;
    let t = app.t();
    // Columns available to a device row: the panel interior minus the
    // highlight symbol.
    let row_room = usize::from(area.width.saturating_sub(2)).saturating_sub(2);
    let items = if app.devices.is_empty() {
        vec![ListItem::new(ratatui::text::Text::from(wrapped_lines(
            t.text(Message::TargetEmpty),
            Style::default().fg(MUTED),
            row_room,
        )))]
    } else {
        app.devices
            .iter()
            .enumerate()
            .map(|(index, device)| {
                let action = if !device.is_eligible_target() {
                    t.text(Message::ActionBlocked).to_string()
                } else if app.selected == Some(index) {
                    t.text(Message::ActionSelected).to_string()
                } else {
                    format!("{} →", t.text(Message::ActionSelect))
                };
                // Path and capacity stay whole, the action stays visible; the
                // name and eligibility text gives way when the row is tight.
                let path = format!("{:<12}", device.path.display());
                let capacity = format!(" {:>9}  ", format_bytes(device.capacity));
                let action = format!("  ·  {action}");
                let middle_room = row_room
                    .saturating_sub(display_width(&path))
                    .saturating_sub(display_width(&capacity))
                    .saturating_sub(display_width(&action));
                let middle = format!(
                    "{}  ·  {}",
                    device.display_name(),
                    target_eligibility_label_in(app.locale, device)
                );
                ListItem::new(Line::from(vec![
                    Span::styled(
                        path,
                        Style::default().fg(if device.is_eligible_target() {
                            ACCENT
                        } else {
                            Color::LightRed
                        }),
                    ),
                    Span::raw(capacity),
                    Span::raw(pad_display(
                        &truncate_end(&middle, middle_room),
                        middle_room,
                    )),
                    Span::raw(action),
                ]))
            })
            .collect::<Vec<_>>()
    };
    let target_title = panel_heading(app.t(), 2, Message::TargetTitle);
    let target_block =
        focused_panel_block(&target_title, app.workspace_focus == WorkspaceFocus::Target);
    let target_inner = target_block.inner(area);
    frame.render_widget(target_block, area);
    let detail_rows = app
        .selected
        .and_then(|index| app.devices.get(index))
        .map(|device| device_details_in(locale, device))
        .filter(|_| target_inner.height >= 9)
        .map(|rows| {
            let join = |labels: &[&str]| {
                rows.iter()
                    .filter(|row| labels.contains(&row.label))
                    .map(|row| format!("{} {}", row.label, row.value))
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            [
                join(&[
                    Message::DetailConnection.text(locale),
                    Message::DetailSerial.text(locale),
                ]),
                join(&[Message::DetailMounted.text(locale)]),
            ]
        });
    let reminder = wrapped_lines(
        t.text(Message::TargetConfirmPhysical),
        Style::default().fg(MUTED),
        usize::from(target_inner.width),
    );
    let reminder_height = reminder.len().min(2) as u16;
    let target_rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(if detail_rows.is_some() { 2 } else { 0 }),
        Constraint::Length(reminder_height),
    ])
    .split(target_inner);
    if let Some(details) = detail_rows {
        frame.render_widget(
            Paragraph::new(details.join("\n"))
                .style(Style::default().fg(MUTED))
                .wrap(Wrap { trim: true }),
            target_rows[2],
        );
    }
    frame.render_widget(
        Paragraph::new(removable_media_status_in(locale, &app.devices))
            .style(Style::default().fg(ACCENT)),
        target_rows[0],
    );
    let mut state = ListState::default().with_selected(app.selected);
    frame.render_stateful_widget(
        List::new(items)
            .style(Style::default().fg(Color::White))
            .highlight_symbol("› ")
            .highlight_style(
                Style::default()
                    .fg(ACCENT)
                    .bg(Color::Rgb(21, 48, 47))
                    .add_modifier(Modifier::BOLD),
            ),
        target_rows[1],
        &mut state,
    );
    frame.render_widget(Paragraph::new(reminder), target_rows[3]);
    app.hit_regions.device_rows = (0..app.devices.len())
        .filter_map(|index| {
            let y = target_rows[1].y.saturating_add(index as u16);
            (y < target_rows[1].bottom()).then_some((
                Rect::new(target_rows[1].x, y, target_rows[1].width, 1),
                index,
            ))
        })
        .collect();
}

fn draw_status(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let t = app.t();
    let review_title = plain_panel_heading(app.t(), 3);
    let status_block =
        focused_panel_block(&review_title, app.workspace_focus == WorkspaceFocus::Review);
    let status_inner = status_block.inner(area);
    frame.render_widget(status_block, area);
    let status_rows = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(3),
    ])
    .split(status_inner);
    let progress_rows =
        if app.download_session.active_progress().is_some() && status_rows[0].height >= 2 {
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(status_rows[0])
        } else {
            Layout::vertical([Constraint::Min(1), Constraint::Length(0)]).split(status_rows[0])
        };
    frame.render_widget(
        Paragraph::new(wrapped_lines(
            &app.status,
            Style::default().fg(MUTED),
            usize::from(progress_rows[0].width),
        )),
        progress_rows[0],
    );
    if let Some(progress) = app.download_session.active_progress() {
        let ratio = progress
            .total
            .filter(|total| *total > 0)
            .map(|total| progress.completed as f64 / total as f64)
            .unwrap_or(0.)
            .clamp(0., 1.);
        frame.render_widget(
            Gauge::default()
                .gauge_style(Style::default().fg(ACCENT).bg(PANEL_SOFT))
                .ratio(ratio)
                .label(format!("{:>5.1}%", ratio * 100.)),
            progress_rows[1],
        );
    }
    let workspace = workspace_progress(
        app.image.as_ref(),
        app.selected.and_then(|index| app.devices.get(index)),
    );
    let help = if status_inner.width >= 100 {
        format!(
            "{}  ·  Tab next · Shift+Tab previous · Enter select · ? help · q quit",
            workspace.status_in(app.locale)
        )
    } else {
        "Tab / Shift+Tab focus · Enter select · ? help · q quit".into()
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::Rgb(111, 130, 153))),
        status_rows[1],
    );
    let compact = status_inner.width < 62;
    if let Some(control) = app.download_session.active_control() {
        let actions = Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
            .spacing(1)
            .split(status_rows[2]);
        let state = control.state();
        render_button(
            frame,
            actions[0],
            &if state == OperationState::Paused {
                format!("▶  {}", t.text(Message::ActionResume))
            } else {
                format!("Ⅱ  {}", t.text(Message::ActionPause))
            },
            state != OperationState::Cancelled,
        );
        render_button(
            frame,
            actions[1],
            &if state == OperationState::Cancelled {
                t.text(Message::ActionCancelling).to_string()
            } else {
                format!("×  {}", t.text(Message::DownloadsActionCancel))
            },
            false,
        );
        app.hit_regions.download_pause = (state != OperationState::Cancelled).then_some(actions[0]);
        app.hit_regions.download_cancel =
            (state != OperationState::Cancelled).then_some(actions[1]);
        app.hit_regions.preview = None;
        app.hit_regions.quit = None;
        return;
    }
    let actions = Layout::horizontal([Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)])
        .spacing(1)
        .split(status_rows[2]);
    let readiness = app.review_readiness();
    let review_label = if readiness == ReviewReadiness::Ready {
        if compact {
            format!("✓ {}", t.text(Message::ActionReview))
        } else {
            format!("✓  {}", readiness.action_label_in(app.locale))
        }
    } else {
        readiness.action_label_in(app.locale).to_string()
    };
    render_button(
        frame,
        actions[0],
        &review_label,
        readiness == ReviewReadiness::Ready,
    );
    render_button(
        frame,
        actions[1],
        &format!(
            "×{}{}",
            if compact { " " } else { "  " },
            t.text(Message::ActionQuit)
        ),
        false,
    );
    app.hit_regions.preview = (readiness == ReviewReadiness::Ready).then_some(actions[0]);
    app.hit_regions.quit = Some(actions[1]);
}

/// `label` made to fit the inside of a bordered button `width` columns wide:
/// first by collapsing the gap after the glyph, then with an ellipsis. A
/// caption is never silently cut mid-word.
fn fit_button_label(label: &str, width: u16) -> String {
    let room = usize::from(width.saturating_sub(2));
    if display_width(label) <= room {
        return label.into();
    }
    truncate_end(&label.replacen("  ", " ", 1), room)
}

fn render_button(frame: &mut ratatui::Frame<'_>, area: Rect, label: &str, primary: bool) {
    let label = fit_button_label(label, area.width);
    let style = if primary {
        Style::default().fg(Color::Black).bg(ACCENT)
    } else {
        Style::default().fg(Color::White).bg(PANEL_SOFT)
    };
    frame.render_widget(
        Paragraph::new(label)
            .alignment(Alignment::Center)
            .style(style)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(if primary { ACCENT } else { BORDER })),
            ),
        area,
    );
}

fn render_disabled_button(frame: &mut ratatui::Frame<'_>, area: Rect, label: &str) {
    frame.render_widget(
        Paragraph::new(fit_button_label(label, area.width))
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED).bg(PANEL_SOFT))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(BORDER)),
            ),
        area,
    );
}

fn render_danger_button(frame: &mut ratatui::Frame<'_>, area: Rect, label: &str) {
    frame.render_widget(
        Paragraph::new(fit_button_label(label, area.width))
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(112, 42, 36)),
            )
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::LightRed)),
            ),
        area,
    );
}

fn render_checkbox(frame: &mut ratatui::Frame<'_>, area: Rect, label: &str, selected: bool) {
    let (marker, style, border) = if selected {
        ("■", Style::default().fg(ACCENT).bg(PANEL_SOFT), ACCENT)
    } else {
        (
            "□",
            Style::default().fg(Color::White).bg(PANEL_SOFT),
            BORDER,
        )
    };
    frame.render_widget(
        Paragraph::new(format!("{marker}  {label}"))
            .alignment(Alignment::Left)
            .style(style)
            .block(
                Block::default()
                    .padding(ratatui::widgets::Padding::horizontal(1))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(border)),
            ),
        area,
    );
}

/// A checkbox that shows the full option wording when the cell has room and
/// the compact caption otherwise (the same concept, never different words).
fn render_option_checkbox(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    label: &str,
    short: &str,
    selected: bool,
) {
    // Borders (2), padding (2), marker and its two spaces (3).
    let room = usize::from(area.width).saturating_sub(7);
    let caption = fit_variant(room, &[label.to_string(), short.to_string()]);
    render_checkbox(frame, area, &truncate_end(&caption, room), selected);
}

fn panel_block<'a>(title: &'a str) -> Block<'a> {
    Block::default()
        .title(title)
        .title_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(PANEL))
}

fn focused_panel_block<'a>(title: &'a str, focused: bool) -> Block<'a> {
    panel_block(title).border_style(Style::default().fg(if focused { ACCENT } else { BORDER }))
}

fn application_area(area: Rect) -> Rect {
    area.inner(ratatui::layout::Margin {
        horizontal: u16::from(area.width >= 72),
        vertical: u16::from(area.height >= 30),
    })
}

fn advanced_height(width: u16) -> u16 {
    if width >= 78 { 9 } else { 17 }
}

fn workspace_height(width: u16) -> u16 {
    if width >= 72 { 13 } else { 24 }
}

fn catalog_min_height(width: u16) -> u16 {
    if width >= 86 { 18 } else { 22 }
}

fn main_shell_layout(area: Rect, header_height: u16, footer_height: u16) -> Vec<Rect> {
    Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(0),
        Constraint::Length(footer_height),
    ])
    .spacing(1)
    .split(area)
    .to_vec()
}

fn centered_button_area(area: Rect) -> Rect {
    if area.height <= 3 {
        return area;
    }
    let y = area.y + area.height.saturating_sub(3) / 2;
    Rect::new(area.x, y, area.width, 3)
}

/// Cycle the Windows partition scheme. Choosing GPT forces the firmware back
/// to UEFI because GPT media cannot boot under legacy BIOS; core stays the
/// final validator.
fn cycle_partition_scheme(options: &mut WriteOptions) {
    options.windows_partition_scheme = match options.windows_partition_scheme {
        bootable_core::WindowsPartitionScheme::Gpt => bootable_core::WindowsPartitionScheme::Mbr,
        bootable_core::WindowsPartitionScheme::Mbr => bootable_core::WindowsPartitionScheme::Gpt,
    };
    if options.windows_partition_scheme == bootable_core::WindowsPartitionScheme::Gpt {
        options.windows_boot_firmware = bootable_core::WindowsBootFirmware::Uefi;
    }
}

/// Cycle the experimental boot firmware in `WindowsBootFirmware::ALL` order.
/// Choosing BIOS + UEFI forces the MBR scheme it requires.
fn cycle_boot_firmware(options: &mut WriteOptions) {
    let all = bootable_core::WindowsBootFirmware::ALL;
    let index = all
        .iter()
        .position(|firmware| *firmware == options.windows_boot_firmware)
        .unwrap_or(0);
    options.windows_boot_firmware = all[(index + 1) % all.len()];
    if options.windows_boot_firmware.includes_legacy_bios() {
        options.windows_partition_scheme = bootable_core::WindowsPartitionScheme::Mbr;
    }
}

/// Widest label that fits the cell, so the experimental marker survives
/// wherever the layout leaves room for it.
fn boot_firmware_label(
    t: Strings,
    firmware: bootable_core::WindowsBootFirmware,
    width: u16,
) -> String {
    let room = usize::from(width.saturating_sub(2));
    let args: [bootable_core::Arg<'_>; 1] = [("value", &firmware)];
    fit_variant(
        room,
        &[
            t.format(Message::OptionsWindowsBootFirmwareValueExperimental, &args),
            t.format(Message::OptionsWindowsBootFirmwareValue, &args),
            t.format(Message::OptionsWindowsBootFirmwareValueCompact, &args),
        ],
    )
}

fn windows_option_columns(width: u16) -> usize {
    match width {
        110.. => 5,
        78.. => 4,
        56.. => 3,
        _ => 2,
    }
}

fn draw_terminal_too_small(frame: &mut ratatui::Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(format!(
            "┌┬┬┐  BOOTABLE v{}\n╰♨─╯\n\nResize to at least 44 × 22\nq  Quit",
            env!("CARGO_PKG_VERSION")
        ))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::White))
        .block(panel_block(" Terminal too small ")),
        area,
    );
}

fn grid_areas(area: Rect, columns: usize, count: usize) -> Vec<Rect> {
    if columns == 0 || count == 0 || area.is_empty() {
        return Vec::new();
    }
    let row_count = count.div_ceil(columns);
    let row_constraints = vec![Constraint::Ratio(1, row_count as u32); row_count];
    Layout::vertical(row_constraints)
        .spacing(u16::from(row_count > 1))
        .split(area)
        .iter()
        .enumerate()
        .flat_map(|(row_index, row)| {
            let items = columns.min(count.saturating_sub(row_index * columns));
            Layout::horizontal(vec![Constraint::Ratio(1, columns as u32); items])
                .spacing(u16::from(items > 1))
                .split(*row)
                .to_vec()
        })
        .take(count)
        .collect()
}

fn contains(area: Option<Rect>, point: (u16, u16)) -> bool {
    area.is_some_and(|area| area.contains(point.into()))
}

fn device_flags(device: &Device) -> String {
    let mut flags = Vec::new();
    if device.removable {
        flags.push("removable");
    }
    if device.read_only {
        flags.push("READ-ONLY");
    }
    if device.system_disk {
        flags.push("SYSTEM—BLOCKED");
    }
    if flags.is_empty() {
        "internal—blocked".into()
    } else {
        flags.join(", ")
    }
}

fn device_change_message(t: Strings, added: usize, removed: usize) -> String {
    match (added, removed) {
        (0, 0) => t.text(Message::StatusDrivesChanged).into(),
        (added, 0) => t.plural(Message::StatusDrivesAdded, added as u64, &[]),
        (0, removed) => t.plural(Message::StatusDrivesRemoved, removed as u64, &[]),
        (added, removed) => t.format(
            Message::StatusDrivesAddedRemoved,
            &[("added", &added), ("removed", &removed)],
        ),
    }
}

#[cfg(test)]
mod layout_tests {
    use super::{
        Cli, Commands, Progress, ProgressPhase, WorkspaceFocus, advanced_height, application_area,
        brand_lockup, centered_button_area, grid_areas, main_shell_layout, progress_event_json,
        windows_option_columns, workspace_height,
    };
    use clap::Parser;
    use ratatui::layout::Rect;

    #[test]
    fn large_terminals_are_not_capped() {
        let area = application_area(Rect::new(0, 0, 180, 60));
        assert_eq!(area, Rect::new(1, 1, 178, 58));
        assert!(area.width > 118);
        assert!(area.height > 39);
    }

    #[test]
    fn compact_terminals_keep_every_available_cell() {
        assert_eq!(
            application_area(Rect::new(0, 0, 60, 20)),
            Rect::new(0, 0, 60, 20)
        );
    }

    #[test]
    fn grids_reflow_without_leaving_their_bounds() {
        let area = Rect::new(2, 4, 60, 7);
        let cells = grid_areas(area, 3, 6);
        assert_eq!(cells.len(), 6);
        assert!(cells.iter().all(|cell| {
            cell.x >= area.x
                && cell.y >= area.y
                && cell.right() <= area.right()
                && cell.bottom() <= area.bottom()
        }));
        assert!(cells[3].y > cells[0].y);
    }

    #[test]
    fn windows_controls_and_advanced_panel_have_breakpoints() {
        assert_eq!(windows_option_columns(120), 5);
        assert_eq!(windows_option_columns(90), 4);
        assert_eq!(windows_option_columns(64), 3);
        assert_eq!(windows_option_columns(44), 2);
        assert_eq!(advanced_height(100), 9);
        assert_eq!(advanced_height(60), 17);
    }

    #[test]
    fn compact_cards_do_not_stretch_with_terminal_height() {
        assert_eq!(workspace_height(120), 13);
        assert_eq!(workspace_height(70), 24);
        assert_eq!(
            centered_button_area(Rect::new(10, 4, 14, 20)),
            Rect::new(10, 12, 14, 3)
        );
    }

    #[test]
    fn main_shell_keeps_header_and_footer_anchored() {
        let area = Rect::new(1, 2, 118, 48);
        let regions = main_shell_layout(area, 5, 7);
        assert_eq!(regions[0], Rect::new(1, 2, 118, 5));
        assert_eq!(regions[2].height, 7);
        assert_eq!(regions[2].bottom(), area.bottom());
        assert_eq!(regions[1].y, regions[0].bottom() + 1);
        assert_eq!(regions[1].bottom() + 1, regions[2].y);
    }

    #[test]
    fn workspace_focus_preserves_order_and_skips_unavailable_setup() {
        assert_eq!(WorkspaceFocus::Source.next(false), WorkspaceFocus::Target);
        assert_eq!(WorkspaceFocus::Target.next(false), WorkspaceFocus::Review);
        assert_eq!(WorkspaceFocus::Target.next(true), WorkspaceFocus::Setup);
        assert_eq!(
            WorkspaceFocus::Review.previous(false),
            WorkspaceFocus::Target
        );
        assert_eq!(WorkspaceFocus::Review.previous(true), WorkspaceFocus::Setup);
    }

    #[test]
    fn terminal_brand_matches_the_download_to_drive_logo() {
        let lines = brand_lockup(
            true,
            "Create boot media",
            "Deliberate writing",
            "Tagline",
            200,
        );
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0]
                .to_string()
                .contains(&format!("┌┬┬┐  BOOTABLE v{}", env!("CARGO_PKG_VERSION")))
        );
        assert!(lines[1].to_string().contains("╰♨─╯"));
    }

    #[test]
    fn write_json_progress_is_an_explicit_client_mode() {
        let cli = Cli::try_parse_from([
            "bootable",
            "write",
            "image.iso",
            "/dev/removable",
            "--confirm",
            "ERASE /dev/removable TEST",
            "--json-progress",
        ])
        .expect("valid client invocation");
        assert!(matches!(
            cli.command,
            Some(Commands::Write {
                json_progress: true,
                ..
            })
        ));
    }

    #[test]
    fn download_json_progress_is_an_explicit_client_mode() {
        let cli = Cli::try_parse_from([
            "bootable",
            "download",
            "cachyos",
            "--index",
            "1",
            "--output",
            "image.iso",
            "--json-progress",
        ])
        .expect("valid catalog client invocation");
        assert!(matches!(
            cli.command,
            Some(Commands::Download {
                json_progress: true,
                ..
            })
        ));
    }

    #[test]
    fn progress_events_are_stable_newline_json_payloads() {
        let event = progress_event_json(&Progress {
            phase: ProgressPhase::Writing,
            completed: 25,
            total: Some(100),
            message: "Writing and verifying".into(),
        });
        let value: serde_json::Value = serde_json::from_str(&event).expect("valid JSON");
        assert_eq!(value["event"], "progress");
        assert_eq!(value["data"]["phase"], "Writing");
        assert_eq!(value["data"]["completed"], 25);
        assert_eq!(value["data"]["total"], 100);
    }
}

#[cfg(test)]
mod workspace_render_tests {
    use super::*;
    use bootable_core::{DeviceId, HELP_SECTIONS, MountPoint, Preferences};
    use ratatui::backend::TestBackend;

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| draw(frame, app)).expect("draw");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app_with_drive() -> App {
        let mut app = App::load(Bootable::native(), None, Picker::halfblocks());
        app.preferences = Preferences::default();
        // Do not depend on the language of the machine running the tests.
        app.locale = Locale::En;
        app.devices = vec![Device {
            id: DeviceId::new("usb-1"),
            path: PathBuf::from("/dev/sdz"),
            vendor: Some("Acme".into()),
            model: Some("Stick".into()),
            serial: Some("ABCDEF123456".into()),
            transport: Some("usb".into()),
            capacity: 16 * 1024 * 1024 * 1024,
            removable: true,
            read_only: false,
            system_disk: false,
            mounts: vec![MountPoint {
                device: PathBuf::from("/dev/sdz1"),
                path: PathBuf::from("/run/media/u/STICK"),
            }],
        }];
        app.selected = Some(0);
        app
    }

    #[test]
    fn selected_drive_details_and_empty_recents_are_visible() {
        let mut app = app_with_drive();
        let screen = render(&mut app, 130, 40);
        assert!(screen.contains("Connection usb"), "{screen}");
        assert!(screen.contains("…3456"), "{screen}");
        assert!(screen.contains("/run/media/u/STICK"), "{screen}");
        assert!(screen.contains("Images you use appear here"), "{screen}");
    }

    #[test]
    fn unselected_drive_shows_no_details() {
        let mut app = app_with_drive();
        app.selected = None;
        let screen = render(&mut app, 130, 40);
        assert!(!screen.contains("…3456"), "{screen}");
    }

    #[test]
    fn guide_lists_every_shared_action() {
        let mut app = app_with_drive();
        app.help_open = true;
        let screen = render(&mut app, 130, 40);
        for section in HELP_SECTIONS {
            assert!(screen.contains(&section.title.to_uppercase()), "{screen}");
        }
    }

    /// Like `render`, but advances by each glyph's display width so wide
    /// characters appear contiguously instead of followed by a blank cell.
    fn render_text(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| draw(frame, app)).expect("draw");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < width {
                    let symbol = buffer[(x, y)].symbol();
                    line.push_str(symbol);
                    x += display_width(symbol).max(1) as u16;
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn localized_app(locale: Locale) -> App {
        let mut app = app_with_drive();
        app.preferences.language = Some(locale);
        app.locale = locale;
        app
    }

    #[test]
    fn workspace_headings_and_language_hint_are_localized() {
        for locale in [Locale::De, Locale::Ru, Locale::Ja] {
            let mut app = localized_app(locale);
            let screen = render_text(&mut app, 130, 40);
            for title in WorkspaceProgress::step_titles(locale) {
                assert!(screen.contains(title), "{locale}: {title}\n{screen}");
            }
            let hint = format!(
                "{}: {}",
                Message::LanguageLabel.text(locale),
                locale.native_name()
            );
            assert!(screen.contains(&hint), "{locale}: {hint}\n{screen}");
            assert!(
                screen.contains(removable_media_status_in(locale, &app.devices).as_str()),
                "{locale}\n{screen}"
            );
            assert!(
                screen.contains(Message::DetailConnection.text(locale)),
                "{locale}\n{screen}"
            );
            assert!(app.hit_regions.language.is_some());
        }
    }

    #[test]
    fn guide_renders_localized_sections_without_breaking_layout() {
        for locale in [Locale::De, Locale::Ru, Locale::Ja] {
            let mut app = localized_app(locale);
            app.help_open = true;
            let screen = render_text(&mut app, 130, 40);
            for section in help_sections(locale) {
                assert!(
                    screen.contains(&section.title.to_uppercase()),
                    "{locale}: {}\n{screen}",
                    section.title
                );
                for entry in &section.entries {
                    assert!(screen.contains(entry.terminal), "{locale}\n{screen}");
                    let first = entry.action.chars().take(6).collect::<String>();
                    assert!(screen.contains(&first), "{locale}: {first}\n{screen}");
                }
            }
            let intro = help_intro(locale).chars().take(8).collect::<String>();
            assert!(screen.contains(&intro), "{locale}\n{screen}");
            // The modal's right border stays in one column on every row.
            let border_columns = screen
                .lines()
                .filter(|line| line.contains('\u{2502}'))
                .filter_map(|line| line.trim_end().rsplit_once('\u{2502}'))
                .map(|(before, _)| display_width(before))
                .collect::<std::collections::BTreeSet<_>>();
            assert!(border_columns.len() <= 2, "{locale}\n{screen}");
        }
    }

    #[test]
    fn wrapping_respects_display_width_in_every_language() {
        let style = Style::default();
        for locale in Locale::available() {
            for section in help_sections(locale) {
                for entry in section.entries {
                    let detail = format!(" \u{b7} {}", entry.detail);
                    for width in [20, 37, 60, 90] {
                        let lines =
                            wrap_styled(&[(entry.action, style), (detail.as_str(), style)], width);
                        for line in lines {
                            let text = line
                                .iter()
                                .map(|span| span.content.as_ref())
                                .collect::<String>();
                            assert!(display_width(&text) <= width, "{locale} {width}: {text:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn wrapped_text_never_loses_characters() {
        let style = Style::default();
        let text = "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}\u{306f}\u{5358}\u{8a9e}\u{306e}\u{9593}\u{306b}\u{7a7a}\u{767d} and mixed words";
        let lines = wrap_styled(&[(text, style)], 16);
        let joined = lines
            .iter()
            .map(|line| {
                line.iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(joined.replace(' ', ""), text.replace(' ', ""));
        assert!(lines.len() > 1);
    }

    #[test]
    fn middle_truncation_counts_columns_not_characters() {
        assert_eq!(truncate_middle("short.iso", 20), "short.iso");
        let wide = "\u{65e5}\u{672c}\u{8a9e}".repeat(4) + ".iso";
        let truncated = truncate_middle(&wide, 11);
        assert!(display_width(&truncated) <= 11, "{truncated}");
        assert!(truncated.contains('\u{2026}'));
        assert_eq!(display_width(&pad_display("\u{65e5}\u{672c}", 6)), 6);
    }

    #[test]
    fn language_cycles_through_system_default_and_every_available_locale() {
        let available = Locale::available();
        let mut current = None;
        let mut seen = Vec::new();
        for _ in 0..=available.len() {
            current = next_language(current);
            seen.push(current);
        }
        let expected = available
            .iter()
            .copied()
            .map(Some)
            .chain([None])
            .collect::<Vec<_>>();
        assert_eq!(seen, expected);
        // A stored language that is no longer offered restarts the cycle.
        assert_eq!(next_language(Some(Locale::Hi)), None);
    }

    #[test]
    fn language_hint_names_system_default_or_the_explicit_choice() {
        assert_eq!(
            language_hint_variants(Some(Locale::De), Locale::De)[0],
            "Sprache: Deutsch"
        );
        assert_eq!(
            language_hint_variants(None, Locale::De)[0],
            "Sprache: Systemstandard (Deutsch)"
        );
        assert_eq!(
            language_hint_variants(None, Locale::En)[0],
            "Language: System default (English)"
        );
    }

    #[test]
    fn english_workspace_headings_are_unchanged() {
        let mut app = app_with_drive();
        let screen = render(&mut app, 130, 40);
        for heading in [
            " 1  Source \u{b7} Choose an image ",
            " 2  Target \u{b7} Choose a drive ",
            " 3  Review & write ",
            "Language: System default (English)",
        ] {
            assert!(screen.contains(heading), "{heading}\n{screen}");
        }
    }

    const LOCALIZED: [Locale; 3] = [Locale::De, Locale::Ru, Locale::Ja];

    /// Leading characters of a catalog message. Long sentences wrap, so only
    /// a prefix that always sits on one row is searched for.
    fn lead(text: &str) -> String {
        text.chars().take(6).collect()
    }

    fn assert_shows(screen: &str, locale: Locale, messages: &[Message]) {
        for message in messages {
            let text = message.text(locale);
            assert!(
                screen.contains(&lead(text)),
                "{locale}: {} = {text:?} is missing\n{screen}",
                message.key()
            );
        }
    }

    fn assert_hides(screen: &str, locale: Locale, english: &[&str]) {
        for phrase in english {
            assert!(
                !screen.contains(phrase),
                "{locale}: untranslated {phrase:?} is still shown\n{screen}"
            );
        }
    }

    fn reviewing_app(locale: Locale) -> App {
        let mut app = localized_app(locale);
        app.image = Some(image_report(bootable_core::ImageKind::HybridIso));
        let device = app.devices[0].clone();
        let plan = bootable_core::WritePlan {
            image: app.image.clone().expect("image"),
            target: device,
            strategy: bootable_core::WriteStrategy::RawVerified,
            options: WriteOptions::default(),
            steps: vec![
                bootable_core::PlanStep {
                    title: "Unmount".into(),
                    destructive: false,
                },
                bootable_core::PlanStep {
                    title: "Write image".into(),
                    destructive: true,
                },
            ],
            required_tools: Vec::new(),
            confirmation_phrase: ERASE_PHRASE.into(),
        };
        app.write_session.open(plan);
        app
    }

    const ERASE_PHRASE: &str = "ERASE /dev/sdz ABCDEF123456";

    #[test]
    fn main_workspace_is_localized_at_130x40() {
        for locale in LOCALIZED {
            let mut app = localized_app(locale);
            let screen = render_text(&mut app, 130, 40);
            assert_shows(
                &screen,
                locale,
                &[
                    Message::HeaderTitleCreate,
                    Message::SourceTitle,
                    Message::TargetTitle,
                    Message::SourceHint,
                    Message::ActionBrowse,
                    Message::SourceRecentEmpty,
                    Message::TargetConfirmPhysical,
                    Message::ActionSelected,
                    Message::DiscoverCollapsedHint,
                ],
            );
            // Buttons use the long wording when it fits and the compact one
            // otherwise; either is the same concept in the same language.
            for (long, compact) in [
                (Message::ActionDownloads, Message::ActionDownloadsCompact),
                (Message::ActionDiscover, Message::ActionDiscoverCompact),
                (Message::ActionRefreshDrives, Message::ActionRefresh),
            ] {
                assert!(
                    screen.contains(long.text(locale)) || screen.contains(compact.text(locale)),
                    "{locale}: {}\n{screen}",
                    long.key()
                );
            }
            assert_hides(
                &screen,
                locale,
                &[
                    "Choose an image",
                    "Choose a drive",
                    "Browse",
                    "Images you use appear here",
                    "Confirm the physical drive",
                    "Selected",
                    "Browse trusted catalogs",
                    "Inspected before writing",
                ],
            );
        }
    }

    #[test]
    fn main_workspace_fits_in_80x30_in_every_language() {
        for locale in Locale::available() {
            let mut app = localized_app(locale);
            let screen = render_text(&mut app, 80, 30);
            assert_shows(
                &screen,
                locale,
                &[
                    Message::SourceTitle,
                    Message::TargetTitle,
                    Message::ActionBrowse,
                ],
            );
            // Nothing is drawn outside the terminal.
            assert!(screen.lines().all(|line| display_width(line) <= 80));
        }
    }

    #[test]
    fn windows_setup_options_are_localized_at_130x40() {
        for locale in LOCALIZED {
            let mut app = localized_app(locale);
            app.image = Some(image_report(windows_kind()));
            app.advanced = true;
            let screen = render_text(&mut app, 130, 40);
            assert_shows(
                &screen,
                locale,
                &[
                    Message::OptionsWindowsTitle,
                    Message::OptionsToolsVerifyImage,
                    Message::OptionsToolsImageFolder,
                    Message::OptionsToolsBackupDrive,
                ],
            );
            // Checkboxes show the full wording or the short caption of the
            // same option, never English.
            for (label, short) in [
                (
                    Message::OptionsWindowsBypassHardwareLabel,
                    Message::OptionsWindowsBypassHardwareShort,
                ),
                (
                    Message::OptionsWindowsOfflineAccountLabel,
                    Message::OptionsWindowsOfflineAccountShort,
                ),
                (
                    Message::OptionsWindowsPrivacyLabel,
                    Message::OptionsWindowsPrivacyShort,
                ),
                (
                    Message::OptionsWindowsBitlockerLabel,
                    Message::OptionsWindowsBitlockerShort,
                ),
            ] {
                assert!(
                    screen.contains(label.text(locale)) || screen.contains(short.text(locale)),
                    "{locale}: {}\n{screen}",
                    label.key()
                );
            }
            assert!(
                screen.contains(&lead(&BadBlockCheck::Disabled.label_in(locale))),
                "{locale}\n{screen}"
            );
            assert_hides(
                &screen,
                locale,
                &[
                    "Hardware bypass",
                    "Offline account",
                    "Privacy defaults",
                    "Disable BitLocker",
                    "Verify image",
                    "Image folder",
                    "Back up drive",
                    "Bad blocks",
                    "Windows installer options",
                ],
            );
        }
    }

    #[test]
    fn windows_option_toggles_report_their_own_localized_status() {
        for locale in LOCALIZED {
            let mut app = localized_app(locale);
            app.image = Some(image_report(windows_kind()));
            app.toggle_windows_requirements();
            assert_eq!(
                app.status,
                Message::OptionsWindowsBypassHardwareOn.text(locale)
            );
            app.toggle_windows_requirements();
            assert_eq!(
                app.status,
                Message::OptionsWindowsBypassHardwareOff.text(locale)
            );
            app.toggle_windows_s_mode();
            assert_eq!(app.status, Message::OptionsWindowsSmodeOn.text(locale));
            app.toggle_windows_qol();
            assert_eq!(app.status, Message::OptionsWindowsQolOn.text(locale));
            app.cycle_windows_partition_scheme();
            assert!(app.status.contains("MBR"), "{}", app.status);
            assert!(
                app.status
                    .starts_with(&lead(Message::StatusWindowsScheme.text(locale))),
                "{locale}: {}",
                app.status
            );
        }
    }

    #[test]
    fn review_screen_is_localized_at_130x40() {
        for locale in LOCALIZED {
            let mut app = reviewing_app(locale);
            let screen = render_text(&mut app, 130, 40);
            assert_shows(
                &screen,
                locale,
                &[
                    Message::ReviewTitle,
                    Message::HeaderSubtitleReview,
                    Message::ReviewPlanSummary,
                    Message::ReviewOrderedOperations,
                    Message::ReviewPermanentChanges,
                    Message::ReviewConsequence,
                    Message::ReviewSubtitle,
                    Message::ActionBack,
                    Message::ReviewActionConsequences,
                    Message::ActionQuit,
                ],
            );
            for heading in [
                Message::ReviewFieldSource,
                Message::ReviewFieldTarget,
                Message::ReviewFieldMethod,
            ] {
                assert!(
                    screen.contains(&locale.strings().heading(heading)),
                    "{locale}: {}\n{screen}",
                    heading.key()
                );
            }
            assert!(
                screen.contains(&locale.strings().heading(Message::ReviewStepErases)),
                "{locale}\n{screen}"
            );
            assert_hides(
                &screen,
                locale,
                &[
                    "Review write plan",
                    "Plan summary",
                    "Ordered operations",
                    "Permanent changes",
                    "Back to selection",
                    "Review consequences",
                    "Nothing is written",
                ],
            );
        }
    }

    #[test]
    fn confirmation_dialog_is_localized_and_never_cut_off() {
        for locale in LOCALIZED {
            for (width, height) in [(130, 40), (80, 30)] {
                let mut app = reviewing_app(locale);
                assert!(app.write_session.open_confirmation());
                let screen = render_text(&mut app, width, height);
                assert_shows(
                    &screen,
                    locale,
                    &[
                        Message::ConfirmTitle,
                        Message::ConfirmPhysicalTarget,
                        Message::ConfirmConsequences,
                        Message::ConfirmConsequenceErase,
                        Message::ConfirmAck,
                        Message::ActionCancel,
                        Message::ConfirmAcknowledgeFirst,
                    ],
                );
                assert!(
                    screen.contains(&locale.strings().heading(Message::ConfirmBadge)),
                    "{locale}\n{screen}"
                );
                // The acknowledgement is the safety-critical sentence: its
                // last words must be on screen, not clipped.
                let ack = Message::ConfirmAck.text(locale);
                let tail = ack
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>();
                assert!(
                    screen.contains(&tail),
                    "{locale} {width}x{height}: ack tail {tail:?} cut off\n{screen}"
                );
                app.write_session.toggle_acknowledged();
                let screen = render_text(&mut app, width, height);
                assert_shows(&screen, locale, &[Message::ConfirmSubmit]);
            }
        }
    }

    #[test]
    fn erase_confirmation_phrase_is_identical_in_every_locale() {
        for locale in Locale::ALL.iter().copied() {
            let mut app = reviewing_app(locale);
            assert!(app.write_session.open_confirmation());
            let review = render_text(&mut app, 130, 40);
            app.write_session.toggle_acknowledged();
            let confirm = render_text(&mut app, 130, 40);
            // The phrase is never part of any screen text, so a translation
            // can neither change nor leak it.
            for screen in [&review, &confirm] {
                assert!(!screen.contains(ERASE_PHRASE), "{locale}\n{screen}");
                assert!(!screen.contains("ERASE /dev"), "{locale}\n{screen}");
            }
            for message in Message::ALL {
                assert!(
                    !message.text(locale).contains("ERASE "),
                    "{locale}: {}",
                    message.key()
                );
            }
            let launch = app.write_session.begin().expect("acknowledged write");
            assert_eq!(launch.confirmation, ERASE_PHRASE, "{locale}");
            assert_eq!(launch.plan.confirmation_phrase, ERASE_PHRASE, "{locale}");
        }
    }

    #[test]
    fn wording_changes_with_the_language_without_rebuilding_the_app() {
        let mut app = localized_app(Locale::En);
        let english = render_text(&mut app, 130, 40);
        assert!(english.contains("Choose an image"), "{english}");
        app.locale = Locale::De;
        let german = render_text(&mut app, 130, 40);
        assert!(german.contains(&lead(Message::SourceTitle.text(Locale::De))));
        assert!(!german.contains("Choose an image"), "{german}");
    }

    #[test]
    fn download_ready_status_survives_localization() {
        for locale in [Locale::En, Locale::De, Locale::Ru, Locale::Ja] {
            let mut app = localized_app(locale);
            let path = PathBuf::from("/tmp/image.iso");

            // Core's final message (it names the integrity result) is kept.
            app.show_download_progress(&Progress {
                phase: ProgressPhase::Finished,
                completed: 1,
                total: Some(1),
                message: "Ready · signature verified · /tmp/image.iso".into(),
            });
            app.show_download_ready(&path);
            assert_eq!(
                app.status, "Ready · signature verified · /tmp/image.iso",
                "{locale}"
            );

            // Without it, the shared (localized) ready line is shown.
            app.show_download_progress(&Progress {
                phase: ProgressPhase::Downloading,
                completed: 1,
                total: Some(2),
                message: "halfway".into(),
            });
            app.show_download_ready(&path);
            assert_eq!(
                app.status,
                locale
                    .strings()
                    .format(Message::StatusDownloadReady, &[("name", &path.display())]),
                "{locale}"
            );
            // The flag is consumed: a later download starts clean.
            assert!(!app.download_final_message);
        }
    }

    #[test]
    fn device_change_statuses_use_the_locale_plural_rules() {
        for locale in [Locale::En, Locale::De, Locale::Ru, Locale::Ja] {
            let t = locale.strings();
            assert_eq!(
                device_change_message(t, 1, 0),
                t.plural(Message::StatusDrivesAdded, 1, &[])
            );
            assert_eq!(
                device_change_message(t, 0, 5),
                t.plural(Message::StatusDrivesRemoved, 5, &[])
            );
            assert_eq!(
                device_change_message(t, 0, 0),
                t.text(Message::StatusDrivesChanged)
            );
        }
        assert_eq!(
            device_change_message(Locale::En.strings(), 2, 1),
            "Drive list changed: 2 added, 1 removed • updated automatically"
        );
    }

    #[test]
    fn setup_options_survive_a_short_terminal_in_every_language() {
        for locale in Locale::available() {
            for kind in [windows_kind(), bootable_core::ImageKind::HybridIso] {
                let mut app = localized_app(locale);
                app.image = Some(image_report(kind));
                app.advanced = true;
                // 80x30 has room for the options panel but not for it and
                // the workspace together; it must not panic or overflow.
                let screen = render_text(&mut app, 80, 30);
                assert_shows(&screen, locale, &[Message::ActionSetupOptions]);
                assert!(screen.lines().all(|line| display_width(line) <= 80));
            }
        }
    }

    #[test]
    fn english_wording_follows_the_unified_catalog() {
        let mut app = app_with_drive();
        let screen = render(&mut app, 130, 40);
        for text in [
            "Create boot media",
            "One deliberate path from image to removable drive.",
            "ISO, IMG, RAW, or compressed disk image",
            "The image is inspected",
            "Images you use appear here for one-click reuse",
            "Confirm the physical drive before continuing",
            "Discover images \u{b7} Browse trusted catalogs \u{b7} Open",
        ] {
            assert!(screen.contains(text), "{text}\n{screen}");
        }
    }

    fn image_report(kind: bootable_core::ImageKind) -> bootable_core::ImageReport {
        bootable_core::ImageReport {
            path: PathBuf::from("/tmp/image.iso"),
            size: 4 * 1024 * 1024 * 1024,
            kind,
            volume_label: None,
            warnings: Vec::new(),
        }
    }

    fn windows_catalog_app(kind: bootable_core::ImageKind) -> App {
        let mut app = app_with_drive();
        app.catalog_open = true;
        app.show_quick_access(QuickAccess::Windows);
        app.image = Some(image_report(kind));
        app
    }

    fn windows_kind() -> bootable_core::ImageKind {
        bootable_core::ImageKind::WindowsInstaller {
            payload: bootable_core::WindowsPayload::Wim,
            payload_size: None,
        }
    }

    #[test]
    fn boot_firmware_control_is_shown_only_for_a_windows_image() {
        let mut app = windows_catalog_app(windows_kind());
        let screen = render(&mut app, 130, 70);
        assert!(screen.contains("Boot firmware: UEFI"), "{screen}");
        assert!(screen.contains("Scheme: GPT"), "{screen}");
        assert!(screen.to_lowercase().contains("experimental"), "{screen}");
        assert!(app.hit_regions.windows_boot_firmware.is_some());

        let mut app = windows_catalog_app(bootable_core::ImageKind::HybridIso);
        let screen = render(&mut app, 130, 70);
        assert!(!screen.contains("Boot firmware"), "{screen}");
        assert!(!screen.to_lowercase().contains("experimental"), "{screen}");
        assert!(app.hit_regions.windows_boot_firmware.is_none());
    }

    #[test]
    fn boot_firmware_key_and_click_cycle_with_coupling() {
        let mut app = windows_catalog_app(windows_kind());
        render(&mut app, 130, 70);
        app.handle_catalog_key(KeyCode::Char('f'));
        assert_eq!(
            app.options.windows_boot_firmware,
            bootable_core::WindowsBootFirmware::BiosAndUefi
        );
        assert_eq!(
            app.options.windows_partition_scheme,
            bootable_core::WindowsPartitionScheme::Mbr
        );
        let screen = render(&mut app, 130, 70);
        assert!(
            screen.contains("Boot firmware: BIOS + UEFI (CSM)"),
            "{screen}"
        );
        assert!(screen.contains("Scheme: MBR"), "{screen}");

        let region = app.hit_regions.windows_boot_firmware.expect("region");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: region.x + 1,
            row: region.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            app.options.windows_boot_firmware,
            bootable_core::WindowsBootFirmware::Uefi
        );
    }

    #[test]
    fn boot_firmware_is_ignored_without_a_windows_image_and_reset_on_new_image() {
        let mut app = windows_catalog_app(bootable_core::ImageKind::HybridIso);
        app.handle_catalog_key(KeyCode::Char('f'));
        assert_eq!(
            app.options.windows_boot_firmware,
            bootable_core::WindowsBootFirmware::Uefi
        );

        let mut app = windows_catalog_app(windows_kind());
        app.handle_catalog_key(KeyCode::Char('f'));
        assert!(app.options.windows_boot_firmware.includes_legacy_bios());
        app.reset_image_scoped_options();
        assert_eq!(
            app.options.windows_boot_firmware,
            bootable_core::WindowsBootFirmware::Uefi
        );
    }

    #[test]
    fn boot_firmware_never_reaches_preferences() {
        let mut app = windows_catalog_app(windows_kind());
        app.handle_catalog_key(KeyCode::Char('f'));
        let saved = serde_json::to_string(&app.preferences).expect("preferences");
        assert!(!saved.to_lowercase().contains("firmware"), "{saved}");
    }

    #[test]
    fn core_refusal_for_bios_on_gpt_keeps_the_review_locked() {
        let mut app = windows_catalog_app(windows_kind());
        // Bypass the TUI coupling to prove core stays the final validator.
        app.options.windows_boot_firmware = bootable_core::WindowsBootFirmware::BiosAndUefi;
        app.options.windows_partition_scheme = bootable_core::WindowsPartitionScheme::Gpt;
        app.preview();
        assert!(
            app.status.contains("requires the MBR partition scheme"),
            "{}",
            app.status
        );
        assert!(!app.write_session.is_reviewing());
    }

    #[test]
    fn coupling_forces_mbr_for_bios_and_uefi_for_gpt() {
        use bootable_core::{WindowsBootFirmware as Firmware, WindowsPartitionScheme as Scheme};
        let mut options = WriteOptions::default();
        assert_eq!(options.windows_boot_firmware, Firmware::Uefi);
        assert_eq!(options.windows_partition_scheme, Scheme::Gpt);

        cycle_boot_firmware(&mut options);
        assert_eq!(options.windows_boot_firmware, Firmware::BiosAndUefi);
        assert_eq!(options.windows_partition_scheme, Scheme::Mbr);

        // Switching to GPT drops legacy BIOS again.
        cycle_partition_scheme(&mut options);
        assert_eq!(options.windows_partition_scheme, Scheme::Gpt);
        assert_eq!(options.windows_boot_firmware, Firmware::Uefi);

        // MBR alone keeps UEFI; cycling firmware back to UEFI keeps MBR.
        cycle_partition_scheme(&mut options);
        assert_eq!(options.windows_partition_scheme, Scheme::Mbr);
        assert_eq!(options.windows_boot_firmware, Firmware::Uefi);
        cycle_boot_firmware(&mut options);
        cycle_boot_firmware(&mut options);
        assert_eq!(options.windows_boot_firmware, Firmware::Uefi);
        assert_eq!(options.windows_partition_scheme, Scheme::Mbr);
    }

    #[test]
    fn boot_firmware_label_degrades_to_fit_narrow_cells() {
        let uefi = bootable_core::WindowsBootFirmware::Uefi;
        assert_eq!(
            boot_firmware_label(Locale::En.strings(), uefi, 60),
            "Boot firmware: UEFI · experimental"
        );
        assert_eq!(
            boot_firmware_label(Locale::En.strings(), uefi, 24),
            "Boot firmware: UEFI"
        );
        assert_eq!(
            boot_firmware_label(Locale::En.strings(), uefi, 10),
            "Firmware: UEFI"
        );
    }
}

#[cfg(test)]
mod cli_tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use anyhow::Result;
    use bootable_core::{
        Device, DeviceId, Error, ImageKind, ImageReport, PlanStep, Progress, WriteOptions,
        WritePlan, WriteStrategy,
    };
    use clap::{CommandFactory, Parser};
    use clap_complete::Shell;

    use super::{
        CAPTURED_EVENTS, Cli, CliError, Commands, ExitStatus, FetchedImage, FlashRequest,
        FlashSource, IntegrityState, ProgressReporter, WriteBackend, classify_source, error_json,
        exit_status, flash_image, integrity_json, wants_json_errors, write_image,
    };

    /// The JSON events a command wrote to stdout on this thread, parsed.
    fn take_events() -> Vec<serde_json::Value> {
        CAPTURED_EVENTS
            .with(|events| std::mem::take(&mut *events.borrow_mut()))
            .iter()
            .map(|line| serde_json::from_str(line).expect("each event is one JSON value"))
            .collect()
    }

    fn event_names(events: &[serde_json::Value]) -> Vec<&str> {
        events
            .iter()
            .map(|event| event["event"].as_str().expect("event name"))
            .collect()
    }

    fn is_terminal(event: &serde_json::Value) -> bool {
        matches!(
            event["event"].as_str(),
            Some("finished" | "failed" | "confirmation_required")
        )
    }

    const PHRASE: &str = "ERASE /dev/fake TEST";

    fn parse(arguments: &[&str]) -> Cli {
        Cli::try_parse_from(arguments).expect("valid invocation")
    }

    fn fake_plan(image: PathBuf, target: &str) -> WritePlan {
        WritePlan {
            image: ImageReport {
                path: image,
                size: 1024,
                kind: ImageKind::HybridIso,
                volume_label: None,
                warnings: Vec::new(),
            },
            target: Device {
                id: DeviceId::new(target),
                path: PathBuf::from(target),
                vendor: None,
                model: None,
                serial: None,
                transport: None,
                capacity: 1 << 30,
                removable: true,
                read_only: false,
                system_disk: false,
                mounts: Vec::new(),
            },
            strategy: WriteStrategy::RawVerified,
            options: WriteOptions::default(),
            steps: vec![PlanStep {
                title: "Write".into(),
                destructive: true,
            }],
            required_tools: Vec::new(),
            confirmation_phrase: PHRASE.into(),
        }
    }

    /// Records every call; never touches a real device or network.
    #[derive(Default)]
    struct FakeBackend {
        eligible: bool,
        write_error: Option<fn() -> Error>,
        fetch_error: Option<fn() -> Error>,
        prepare_error: Option<fn() -> Error>,
        integrity: Option<IntegrityState>,
        fetches: RefCell<Vec<String>>,
        prepares: RefCell<Vec<(PathBuf, String)>>,
        writes: RefCell<Vec<String>>,
    }

    impl FakeBackend {
        fn eligible() -> Self {
            Self {
                eligible: true,
                ..Self::default()
            }
        }
    }

    impl WriteBackend for FakeBackend {
        fn check_target(&self, target: &str) -> Result<()> {
            if self.eligible {
                Ok(())
            } else {
                Err(Error::UnsafeTarget(format!("{target}: Internal disk · blocked")).into())
            }
        }

        fn fetch(
            &self,
            slug: &str,
            _index: usize,
            _output: Option<PathBuf>,
            _progress: &mut dyn FnMut(Progress),
        ) -> Result<FetchedImage> {
            self.fetches.borrow_mut().push(slug.into());
            if let Some(error) = self.fetch_error {
                return Err(error().into());
            }
            Ok(FetchedImage {
                report: fake_plan(PathBuf::from(format!("{slug}.iso")), "/dev/fake").image,
                integrity: self
                    .integrity
                    .clone()
                    .unwrap_or(IntegrityState::TransferChecked),
            })
        }

        fn prepare(
            &self,
            image: PathBuf,
            target: &str,
            _options: WriteOptions,
        ) -> Result<WritePlan> {
            if let Some(error) = self.prepare_error {
                return Err(error().into());
            }
            self.prepares
                .borrow_mut()
                .push((image.clone(), target.into()));
            Ok(fake_plan(image, target))
        }

        fn write(
            &self,
            _plan: &WritePlan,
            confirmation: &str,
            _progress: &mut dyn FnMut(Progress),
        ) -> Result<()> {
            self.writes.borrow_mut().push(confirmation.into());
            match self.write_error {
                Some(error) => Err(error().into()),
                None => Ok(()),
            }
        }
    }

    fn request(source: &str, confirm: Option<&str>) -> FlashRequest {
        FlashRequest {
            source: source.into(),
            target: "/dev/fake".into(),
            index: 0,
            output: None,
            require_signature: false,
            confirm: confirm.map(Into::into),
            json_progress: true,
            options: WriteOptions::default(),
        }
    }

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(ExitStatus::Ok.code(), 0);
        assert_eq!(ExitStatus::Error.code(), 1);
        assert_eq!(ExitStatus::Usage.code(), 2);
        assert_eq!(ExitStatus::Confirmation.code(), 3);
        assert_eq!(ExitStatus::Verification.code(), 4);
    }

    #[test]
    fn errors_map_to_documented_exit_codes() {
        let status = |error: anyhow::Error| exit_status(&error);
        assert_eq!(
            status(CliError::Usage("bad".into()).into()),
            ExitStatus::Usage
        );
        assert_eq!(
            status(CliError::ConfirmationRequired { phrase: "x".into() }.into()),
            ExitStatus::Confirmation
        );
        assert_eq!(
            status(
                Error::ConfirmationMismatch {
                    expected: "x".into()
                }
                .into()
            ),
            ExitStatus::Confirmation
        );
        assert_eq!(
            status(Error::UnsafeTarget("system disk".into()).into()),
            ExitStatus::Confirmation
        );
        assert_eq!(
            status(Error::StalePlan("verification failed: SHA-256 digests differ".into()).into()),
            ExitStatus::Verification
        );
        assert_eq!(
            status(Error::InvalidDownload("SHA-256 checksum mismatch for x.iso".into()).into()),
            ExitStatus::Verification
        );
        assert_eq!(
            status(Error::StalePlan("the device changed".into()).into()),
            ExitStatus::Error
        );
        assert_eq!(status(Error::NotPrivileged.into()), ExitStatus::Error);
        assert_eq!(status(anyhow::anyhow!("boom")), ExitStatus::Error);
        // Context added by callers must not hide the underlying status.
        let wrapped = anyhow::Error::from(Error::UnsafeTarget("x".into())).context("planning");
        assert_eq!(exit_status(&wrapped), ExitStatus::Confirmation);
    }

    #[test]
    fn json_errors_share_one_shape() {
        let value = error_json("nope", ExitStatus::Verification);
        assert_eq!(value["error"]["kind"], "verification_failed");
        assert_eq!(value["error"]["exit_code"], 4);
        assert_eq!(value["error"]["message"], "nope");
        let with_json = parse(&["bootable", "devices", "--json"]);
        assert!(wants_json_errors(&with_json.command.expect("command")));
        let without_json = parse(&["bootable", "devices"]);
        assert!(!wants_json_errors(&without_json.command.expect("command")));
    }

    #[test]
    fn flash_parses_with_explicit_target_and_options() {
        let cli = parse(&[
            "bootable",
            "flash",
            "cachyos",
            "/dev/sdx",
            "--index",
            "2",
            "--output",
            "a.iso",
            "--confirm",
            PHRASE,
            "--json-progress",
        ]);
        let Some(Commands::Flash {
            source,
            target,
            index,
            output,
            confirm,
            json_progress,
            ..
        }) = cli.command
        else {
            panic!("expected flash");
        };
        assert_eq!(source, "cachyos");
        assert_eq!(target, "/dev/sdx");
        assert_eq!(index, 2);
        assert_eq!(output, Some(PathBuf::from("a.iso")));
        assert_eq!(confirm.as_deref(), Some(PHRASE));
        assert!(json_progress);
    }

    #[test]
    fn flash_never_infers_a_target() {
        assert!(Cli::try_parse_from(["bootable", "flash", "cachyos"]).is_err());
        assert!(Cli::try_parse_from(["bootable", "flash"]).is_err());
    }

    #[test]
    fn completions_parse_every_supported_shell() {
        for name in ["bash", "zsh", "fish", "powershell", "elvish"] {
            let cli = parse(&["bootable", "completions", name]);
            assert!(matches!(cli.command, Some(Commands::Completions { .. })));
        }
        assert!(Cli::try_parse_from(["bootable", "completions", "tcsh"]).is_err());
    }

    #[test]
    fn completions_scripts_mention_flash() {
        for shell in [
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::PowerShell,
            Shell::Elvish,
        ] {
            let mut output = Vec::new();
            clap_complete::generate(shell, &mut Cli::command(), "bootable", &mut output);
            let script = String::from_utf8(output).expect("utf-8 script");
            assert!(script.contains("flash"), "{shell} completions lack flash");
        }
    }

    #[test]
    fn sources_are_classified_as_paths_or_slugs() {
        assert_eq!(
            classify_source("cachyos", false),
            FlashSource::Catalog("cachyos".into())
        );
        for path in ["image.iso", "./image", "/tmp/x", "dir\\x", "~/x"] {
            assert_eq!(
                classify_source(path, false),
                FlashSource::Image(PathBuf::from(path))
            );
        }
        assert_eq!(
            classify_source("local", true),
            FlashSource::Image(PathBuf::from("local"))
        );
    }

    #[test]
    fn flash_without_confirm_plans_but_never_writes() {
        let backend = FakeBackend::eligible();
        let error = flash_image(&backend, request("missing.iso", None)).expect_err("refused");
        assert_eq!(exit_status(&error), ExitStatus::Confirmation);
        assert!(error.to_string().contains(PHRASE));
        assert_eq!(backend.prepares.borrow().len(), 1);
        assert!(backend.writes.borrow().is_empty());
    }

    #[test]
    fn flash_with_wrong_phrase_never_writes() {
        let backend = FakeBackend::eligible();
        let error =
            flash_image(&backend, request("missing.iso", Some("yes"))).expect_err("refused");
        assert_eq!(exit_status(&error), ExitStatus::Confirmation);
        assert!(backend.writes.borrow().is_empty());
    }

    #[test]
    fn flash_writes_only_with_the_exact_phrase() {
        let backend = FakeBackend::eligible();
        flash_image(&backend, request("missing.iso", Some(PHRASE))).expect("flashed");
        assert_eq!(*backend.writes.borrow(), vec![PHRASE.to_owned()]);
        assert_eq!(
            backend.prepares.borrow()[0],
            (PathBuf::from("missing.iso"), "/dev/fake".to_owned())
        );
        assert!(backend.fetches.borrow().is_empty());
    }

    #[test]
    fn flash_slug_fetches_then_plans_the_downloaded_image() {
        let backend = FakeBackend::eligible();
        flash_image(&backend, request("cachyos", Some(PHRASE))).expect("flashed");
        assert_eq!(*backend.fetches.borrow(), vec!["cachyos".to_owned()]);
        assert_eq!(backend.prepares.borrow()[0].0, PathBuf::from("cachyos.iso"));
        assert_eq!(backend.writes.borrow().len(), 1);
    }

    #[test]
    fn flash_refuses_ineligible_target_before_downloading() {
        let backend = FakeBackend::default();
        let error = flash_image(&backend, request("cachyos", Some(PHRASE))).expect_err("refused");
        assert_eq!(exit_status(&error), ExitStatus::Confirmation);
        assert!(backend.fetches.borrow().is_empty());
        assert!(backend.prepares.borrow().is_empty());
        assert!(backend.writes.borrow().is_empty());
    }

    #[test]
    fn flash_reports_verification_failure_with_its_own_exit_code() {
        let backend = FakeBackend {
            eligible: true,
            write_error: Some(|| Error::StalePlan("verification failed: digests differ".into())),
            ..FakeBackend::default()
        };
        let error =
            flash_image(&backend, request("missing.iso", Some(PHRASE))).expect_err("failed");
        assert_eq!(exit_status(&error), ExitStatus::Verification);
    }

    #[test]
    fn write_shares_the_confirmation_gate() {
        let backend = FakeBackend::eligible();
        let error = write_image(
            &backend,
            PathBuf::from("image.iso"),
            "/dev/fake",
            None,
            true,
            WriteOptions::default(),
        )
        .expect_err("refused");
        assert_eq!(exit_status(&error), ExitStatus::Confirmation);
        assert!(backend.writes.borrow().is_empty());
    }

    #[test]
    fn windows_boot_firmware_flag_defaults_to_uefi_and_parses_on_every_write_command() {
        use bootable_core::WindowsBootFirmware as Firmware;
        fn firmware(cli: Cli) -> Firmware {
            match cli.command {
                Some(
                    Commands::Plan { windows, .. }
                    | Commands::Write { windows, .. }
                    | Commands::Flash { windows, .. },
                ) => windows.windows_boot_firmware,
                other => panic!("unexpected command {other:?}"),
            }
        }
        assert_eq!(
            firmware(parse(&["bootable", "plan", "w.iso", "/dev/x"])),
            Firmware::Uefi
        );
        for command in ["plan", "write"] {
            let cli = parse(&[
                "bootable",
                command,
                "w.iso",
                "/dev/x",
                "--windows-partition-scheme",
                "mbr",
                "--windows-boot-firmware",
                "bios-uefi",
            ]);
            assert_eq!(firmware(cli), Firmware::BiosAndUefi, "{command}");
        }
        let cli = parse(&[
            "bootable",
            "flash",
            "w.iso",
            "/dev/x",
            "--windows-boot-firmware",
            "uefi",
        ]);
        assert_eq!(firmware(cli), Firmware::Uefi);
        assert!(
            Cli::try_parse_from([
                "bootable",
                "plan",
                "w.iso",
                "/dev/x",
                "--windows-boot-firmware",
                "efi"
            ])
            .is_err()
        );
    }

    fn download_refusal() -> Error {
        Error::InvalidDownload(
            "a verified publisher signature is required but this image has only: Publisher \
             checksum verified"
                .into(),
        )
    }

    #[test]
    fn require_signature_parses_on_download_and_flash_and_defaults_off() {
        let on = |cli: Cli| match cli.command {
            Some(
                Commands::Download {
                    require_signature, ..
                }
                | Commands::Flash {
                    require_signature, ..
                },
            ) => require_signature,
            other => panic!("unexpected command {other:?}"),
        };
        assert!(!on(parse(&["bootable", "download", "ubuntu"])));
        assert!(on(parse(&[
            "bootable",
            "download",
            "ubuntu",
            "--require-signature"
        ])));
        assert!(!on(parse(&["bootable", "flash", "ubuntu", "/dev/x"])));
        assert!(on(parse(&[
            "bootable",
            "flash",
            "ubuntu",
            "/dev/x",
            "--require-signature",
            "--json-progress"
        ])));
        // Only the commands that fetch catalog images take the flag.
        assert!(
            Cli::try_parse_from([
                "bootable",
                "write",
                "a.iso",
                "/dev/x",
                "--require-signature"
            ])
            .is_err()
        );
    }

    #[test]
    fn signature_refusals_exit_with_the_verification_code() {
        assert_eq!(
            exit_status(&download_refusal().into()),
            ExitStatus::Verification
        );
        let rejected = Error::InvalidDownload(
            "signature verification failed for https://example.org/SHA256SUMS".into(),
        );
        assert_eq!(exit_status(&rejected.into()), ExitStatus::Verification);
        // An unrelated refusal keeps the generic code.
        assert_eq!(
            exit_status(&Error::InvalidDownload("not an ISO".into()).into()),
            ExitStatus::Error
        );
    }

    #[test]
    fn json_progress_signature_refusal_emits_one_failed_event_with_code_4() {
        take_events();
        let backend = FakeBackend {
            eligible: true,
            fetch_error: Some(download_refusal),
            ..FakeBackend::default()
        };
        let mut flash = request("ubuntu", Some(PHRASE));
        flash.require_signature = true;
        let error = flash_image(&backend, flash).expect_err("refused");
        assert_eq!(exit_status(&error), ExitStatus::Verification);
        assert!(backend.prepares.borrow().is_empty());
        assert!(backend.writes.borrow().is_empty());
        let events = take_events();
        assert_eq!(event_names(&events), ["failed"]);
        assert_eq!(events[0]["data"]["kind"], "verification_failed");
        assert_eq!(events[0]["data"]["exit_code"], 4);
        assert!(
            events[0]["data"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("signature is required"))
        );
    }

    #[test]
    fn require_signature_with_a_local_image_is_a_usage_error() {
        take_events();
        let backend = FakeBackend::eligible();
        let mut flash = request("local.iso", Some(PHRASE));
        flash.require_signature = true;
        let error = flash_image(&backend, flash).expect_err("refused");
        assert_eq!(exit_status(&error), ExitStatus::Usage);
        assert!(backend.prepares.borrow().is_empty());
        assert!(backend.writes.borrow().is_empty());
        let events = take_events();
        assert_eq!(event_names(&events), ["failed"]);
        assert_eq!(events[0]["data"]["exit_code"], 2);
    }

    #[test]
    fn early_json_progress_failures_emit_exactly_one_terminal_event() {
        let ineligible = FakeBackend::default();
        let prepare_fails = FakeBackend {
            eligible: true,
            prepare_error: Some(|| Error::StalePlan("the device changed".into())),
            ..FakeBackend::default()
        };
        let unsafe_target = FakeBackend {
            eligible: true,
            prepare_error: Some(|| Error::UnsafeTarget("/dev/sda: system disk".into())),
            ..FakeBackend::default()
        };
        let cases: [(&str, &FakeBackend, ExitStatus); 3] = [
            ("cachyos", &ineligible, ExitStatus::Confirmation),
            ("local.iso", &prepare_fails, ExitStatus::Error),
            ("local.iso", &unsafe_target, ExitStatus::Confirmation),
        ];
        for (source, backend, status) in cases {
            take_events();
            let error = flash_image(backend, request(source, Some(PHRASE))).expect_err("fails");
            assert_eq!(exit_status(&error), status, "{source}");
            let events = take_events();
            assert_eq!(
                events.iter().filter(|event| is_terminal(event)).count(),
                1,
                "{source}: {events:?}"
            );
            let last = events.last().expect("an event");
            assert_eq!(last["event"], "failed");
            assert_eq!(last["data"]["kind"], status.name());
            assert_eq!(last["data"]["exit_code"], status.code());
        }
        // `write` reports a failed plan the same way.
        take_events();
        let error = write_image(
            &prepare_fails,
            PathBuf::from("image.iso"),
            "/dev/fake",
            Some(PHRASE.into()),
            true,
            WriteOptions::default(),
        )
        .expect_err("fails");
        assert_eq!(exit_status(&error), ExitStatus::Error);
        let events = take_events();
        assert_eq!(event_names(&events), ["failed"]);
    }

    #[test]
    fn json_progress_streams_end_in_exactly_one_terminal_event() {
        // Success.
        take_events();
        flash_image(&FakeBackend::eligible(), request("cachyos", Some(PHRASE))).expect("flashed");
        let events = take_events();
        assert_eq!(event_names(&events), ["integrity", "finished"]);
        // No confirmation: the plan event is the terminal one.
        flash_image(&FakeBackend::eligible(), request("cachyos", None)).expect_err("needs phrase");
        let events = take_events();
        assert_eq!(event_names(&events), ["integrity", "confirmation_required"]);
        // Wrong phrase.
        flash_image(&FakeBackend::eligible(), request("local.iso", Some("no"))).expect_err("bad");
        let events = take_events();
        assert_eq!(event_names(&events), ["failed"]);
        assert_eq!(events[0]["data"]["exit_code"], 3);
    }

    #[test]
    fn integrity_json_has_a_stable_shape() {
        let transfer = integrity_json(&IntegrityState::TransferChecked);
        assert_eq!(transfer["label"], IntegrityState::TransferChecked.label());
        assert_eq!(transfer["signature_verified"], false);
        assert_eq!(transfer["signature_expected_but_unverified"], false);
        assert_eq!(transfer.as_object().expect("object").len(), 3);

        let downgraded = IntegrityState::ChecksumVerified {
            algorithm: bootable_core::ChecksumAlgorithm::Sha256,
            signature_note: Some("signature expected but unavailable".into()),
            signature_expected: true,
        };
        let value = integrity_json(&downgraded);
        assert_eq!(value["label"], downgraded.label());
        assert!(
            value["label"]
                .as_str()
                .is_some_and(|label| label.contains("signature expected but unavailable"))
        );
        assert_eq!(value["signature_verified"], false);
        assert_eq!(value["signature_expected_but_unverified"], true);
    }

    #[test]
    fn integrity_event_carries_the_state_of_the_fetched_image() {
        take_events();
        let downgraded = IntegrityState::ChecksumVerified {
            algorithm: bootable_core::ChecksumAlgorithm::Sha256,
            signature_note: Some("signature expected but unavailable".into()),
            signature_expected: true,
        };
        let backend = FakeBackend {
            eligible: true,
            integrity: Some(downgraded.clone()),
            ..FakeBackend::default()
        };
        flash_image(&backend, request("ubuntu", Some(PHRASE))).expect("flashed");
        let events = take_events();
        assert_eq!(events[0]["event"], "integrity");
        assert_eq!(events[0]["data"], integrity_json(&downgraded));
        ProgressReporter::new(true).integrity(&IntegrityState::TransferChecked);
        let events = take_events();
        assert_eq!(events[0]["data"]["signature_verified"], false);
    }

    #[test]
    fn windows_boot_firmware_flag_reaches_write_options() {
        let cli = parse(&[
            "bootable",
            "plan",
            "w.iso",
            "/dev/x",
            "--windows-boot-firmware",
            "bios-uefi",
        ]);
        let Some(Commands::Plan { windows, .. }) = cli.command else {
            panic!("expected plan");
        };
        let options = super::write_options(windows, Default::default());
        assert!(options.windows_boot_firmware.includes_legacy_bios());
    }
}
