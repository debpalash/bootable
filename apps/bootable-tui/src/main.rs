use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bootable_core::{
    BadBlockCheck, Bootable, CacheMode, CatalogFacet, CatalogFetch, CatalogState,
    ChecksumAlgorithm, Device, DiscoverySession, DiscoverySource, DistributionBundle,
    DistributionDetails, DistributionSummary, DownloadCompletion, DownloadLaunch, DownloadRequest,
    DownloadStatus, ImageReport, IsoRelease, ManagedDownloadSession, OperationState, PiCatalog,
    Progress, ProgressPhase, QuickAccess, ReviewReadiness, ReviewedWriteSession,
    WorkspaceStepState, WriteCompletion, WriteOptions, WritePlan, format_bytes, review_readiness,
    target_eligibility_label, workspace_progress,
};
use clap::{Args, Parser, Subcommand};
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
}

#[derive(Debug, Args)]
struct WindowsArgs {
    #[arg(long, default_value = "gpt", value_name = "gpt|mbr")]
    windows_partition_scheme: bootable_core::WindowsPartitionScheme,
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let engine = Bootable::native();
    match cli.command {
        Some(Commands::Catalog { limit, json }) => print_catalog(&engine, limit, json),
        Some(Commands::Releases { slug, json }) => print_releases(&engine, &slug, json),
        Some(Commands::Download {
            slug,
            index,
            output,
        }) => download_release(&engine, &slug, index, output),
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
        None if io::stdout().is_terminal() => run_tui(engine, cli.image),
        None => bail!("interactive mode needs a terminal; use `bootable --help`"),
    }
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
) -> Result<()> {
    let details = engine.distribution_details(slug)?;
    let releases = resolve_releases(engine, &details)?;
    let release = releases
        .get(index)
        .with_context(|| format!("release index {index} is out of range"))?;
    let destination = output.unwrap_or_else(|| PathBuf::from(&release.name));
    let mut reporter = ProgressReporter::default();
    let report = engine.download_iso(release, &destination, |progress| reporter.print(progress))?;
    println!("Ready to write: {}", report.path.display());
    println!("Kind: {}", report.kind);
    println!("Size: {}", format_bytes(report.size));
    Ok(())
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

fn write_image(
    engine: &Bootable,
    image: PathBuf,
    target: &str,
    confirmation: Option<String>,
    json_progress: bool,
    options: WriteOptions,
) -> Result<()> {
    let plan = engine.prepare_with_options(image, target, options)?;
    let Some(confirmation) = confirmation else {
        render_plan_text(&plan);
        bail!(
            "nothing was written; repeat with --confirm '{}'",
            plan.confirmation_phrase
        );
    };
    let mut reporter = ProgressReporter::new(json_progress);
    let result =
        engine.write_with_privilege(&plan, &confirmation, |progress| reporter.print(progress));
    match result {
        Ok(()) => {
            reporter.finished();
            Ok(())
        }
        Err(error) => {
            reporter.failed(&error.to_string());
            Err(error.into())
        }
    }
}

fn write_options(windows: WindowsArgs, bad_block_check: BadBlockCheck) -> WriteOptions {
    WriteOptions {
        windows_partition_scheme: windows.windows_partition_scheme,
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
            println!("{}", progress_event_json(&progress));
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
            println!("{{\"event\":\"finished\"}}");
        }
    }

    fn failed(&self, message: &str) {
        if self.json {
            println!(
                "{}",
                serde_json::json!({ "event": "failed", "data": { "message": message } })
            );
        }
    }
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

struct App {
    engine: Bootable,
    devices: Vec<Device>,
    image: Option<ImageReport>,
    image_loading: bool,
    image_receiver: Option<Receiver<std::result::Result<(ImageReport, PathBuf), String>>>,
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
    fn load(engine: Bootable, image_path: Option<PathBuf>, artwork_picker: Picker) -> Self {
        let devices_result = engine.discover_devices();
        let (devices, status) = match devices_result {
            Ok(devices) => {
                let eligible = devices
                    .iter()
                    .filter(|device| device.is_eligible_target())
                    .count();
                (
                    devices,
                    format!("{eligible} eligible target(s) detected · choose an image to begin"),
                )
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
            checksum_algorithm: ChecksumAlgorithm::Sha256,
            browse_directory: None,
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
        }
    }

    fn load_distrowatch(&mut self) {
        self.load_distrowatch_with(CacheMode::PreferCache);
    }

    fn load_distrowatch_with(&mut self, mode: CacheMode) {
        self.discovery_session.show_distrowatch(QuickAccess::All);
        self.catalog_focus = CatalogFocus::Distributions;
        if !self.popular_distributions.is_empty() && mode == CacheMode::PreferCache {
            self.distributions = self.popular_distributions.clone();
            self.status = "DistroWatch discovery selected".into();
        } else if self.discovery_session.begin(CatalogFacet::Popular) {
            self.status = "Loading distributions…".into();
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
            self.status = "Raspberry Pi image discovery selected".into();
            return;
        }
        if !self.discovery_session.begin(CatalogFacet::RaspberryPi) {
            return;
        }
        self.status = "Loading Raspberry Pi images…".into();
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
                self.status = "Showing DistroWatch six-month popularity".into();
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
                    self.status = "Omarchy quick access · press Enter to resolve ISOs".into();
                } else {
                    self.status =
                        "Omarchy is missing from the current DistroWatch directory".into();
                }
            }
            QuickAccess::Windows => {
                self.distributions.clear();
                self.status = "Windows media tools · press o to choose a Windows ISO".into();
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
        self.status = format!("Loading {base}-based distributions…");
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

    fn toggle_catalog(&mut self) {
        if self.catalog_open {
            self.catalog_open = false;
            self.status = format!("Catalog closed • {}", self.review_readiness().guidance());
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
        self.status = format!("Loading {} releases…", distribution.name);
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
                self.status = "Windows tools use the selected local ISO".into();
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
        while let Ok(update) = self.catalog_receiver.try_recv() {
            match update {
                CatalogUpdate::Popular(result) => match result {
                    Ok(fetch) => {
                        self.discovery_session.complete(
                            CatalogFacet::Popular,
                            &fetch,
                            fetch.value.is_empty(),
                        );
                        let source = fetch.status_suffix();
                        let distributions = fetch.value;
                        let count = distributions.len();
                        self.popular_distributions = distributions.clone();
                        if self.discovery_session.source() == DiscoverySource::DistroWatch
                            && self.discovery_session.quick_access() == QuickAccess::All
                            && self.catalog_query.is_empty()
                        {
                            self.distributions = distributions;
                            self.catalog_selected = 0;
                            self.status = format!("{count} distributions · {source}");
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
                            .short_label("distributions");
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
                            self.status = "Search catalog ready".into();
                        }
                    }
                    Err(error) => {
                        self.discovery_session
                            .fail(CatalogFacet::Directory, error.clone());
                        if !self.catalog_query.is_empty() {
                            self.status = self
                                .discovery_session
                                .state(CatalogFacet::Directory)
                                .short_label("search catalog");
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
                        let source = fetch.status_suffix();
                        let catalog = fetch.value;
                        let count = catalog.images.len();
                        self.pi_catalog = Some(catalog);
                        self.pi_device_selected = 0;
                        self.pi_image_selected = 0;
                        if self.discovery_session.source() == DiscoverySource::RaspberryPi {
                            self.status = format!("{count} Raspberry Pi images · {source}");
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
                            .short_label("Raspberry Pi images");
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
                        let source = fetch.status_suffix();
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
                            self.status = format!("{count} {base}-based distributions · {source}");
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
                            self.status = self
                                .discovery_session
                                .state(facet)
                                .short_label(&format!("{base}-based distributions"));
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
                            let source = fetch.status_suffix();
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
                            self.status = if count == 0 && !warnings.is_empty() {
                                format!(
                                    "Profile ready · no direct ISO found · {} source error(s)",
                                    warnings.len()
                                )
                            } else if count == 0 {
                                "Profile ready · no direct ISO found".into()
                            } else if !warnings.is_empty() {
                                format!(
                                    "{count} ISO release(s) · {source} · {} source warning(s)",
                                    warnings.len()
                                )
                            } else {
                                format!("{count} ISO release(s) · {source}")
                            };
                        }
                        Err(error) => {
                            self.discovery_session.fail(CatalogFacet::Details, error);
                            self.status = self
                                .discovery_session
                                .state(CatalogFacet::Details)
                                .short_label("ISO releases");
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
                                self.artwork_error =
                                    Some(format!("Could not decode catalog artwork: {error}"));
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
            Err(error) => self.status = format!("Download history unavailable · {error}"),
        }
    }

    fn toggle_downloads(&mut self) {
        self.downloads_open = !self.downloads_open;
        if self.downloads_open {
            self.refresh_download_jobs();
            self.status = format!(
                "{} managed download job(s)",
                self.download_session.jobs().len()
            );
        }
    }

    fn launch_download_job(&mut self, id: String, destination: PathBuf, retry: bool) {
        let DownloadRequest::Launch(launch) = self.download_session.request(id, destination, retry)
        else {
            self.status = "Download queued · it starts when the active job finishes".into();
            self.refresh_download_jobs();
            return;
        };
        self.launch_download_worker(launch);
    }

    fn launch_download_worker(&mut self, launch: DownloadLaunch) {
        self.status = if launch.retry {
            "Retrying download · preserved bytes resume when supported".into()
        } else {
            "Starting managed download…".into()
        };
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
            self.status = "Choose a download job first".into();
            return;
        };
        let id = job.id.clone();
        match self.download_session.retry(&self.engine, &id) {
            Ok(DownloadRequest::Launch(launch)) => self.launch_download_worker(launch),
            Ok(DownloadRequest::Queued) => {
                self.status = "Retry queued · it starts after the active download".into()
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    fn start_next_queued_download(&mut self) {
        match self.download_session.next_queued(&self.engine) {
            Ok(Some(launch)) => self.launch_download_worker(launch),
            Ok(None) => {}
            Err(error) => self.status = format!("Could not start queued download · {error}"),
        }
    }

    fn use_selected_download(&mut self) {
        let Some(job) = self.download_session.jobs().get(self.download_selected) else {
            self.status = "Choose a download job first".into();
            return;
        };
        let id = job.id.clone();
        let destination = job.destination.clone();
        match self.download_session.use_completed(&self.engine, &id) {
            Ok(report) => {
                self.browse_directory = destination.parent().map(PathBuf::from);
                self.image = Some(report);
                self.advanced = false;
                self.downloads_open = false;
                self.status = format!("Using completed download {}", destination.display());
            }
            Err(error) => self.status = format!("Downloaded image is unavailable · {error}"),
        }
    }

    fn remove_selected_download(&mut self) {
        let Some(job) = self.download_session.jobs().get(self.download_selected) else {
            self.status = "Choose a download job first".into();
            return;
        };
        let id = job.id.clone();
        match self.download_session.remove(&self.engine, &id) {
            Ok(()) => {
                self.status = "History entry removed · completed image kept".into();
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
            self.status = "Choose an ISO release first".into();
            return;
        };
        let mut dialog = rfd::FileDialog::new()
            .add_filter("ISO images", &["iso"])
            .set_file_name(&release.name);
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        let Some(destination) = dialog.save_file() else {
            self.status = "ISO download cancelled".into();
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
            self.status = "Choose a distribution first".into();
            return;
        };
        self.status = match self.engine.open_distrowatch_page(&page_url) {
            Ok(()) => "Opened the DistroWatch distribution page in your browser".into(),
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
            self.status = "Choose a Raspberry Pi image first".into();
            return;
        };
        let mut dialog = rfd::FileDialog::new().set_file_name(&image.suggested_filename);
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        let Some(destination) = dialog.save_file() else {
            self.status = "Raspberry Pi image download cancelled".into();
            return;
        };
        match self.engine.enqueue_pi_download(&image, &destination) {
            Ok(id) => self.launch_download_job(id, destination, false),
            Err(error) => self.status = error.to_string(),
        }
    }

    fn poll_download(&mut self) {
        let Some(receiver) = self.download_receiver.take() else {
            return;
        };
        let mut finished = false;
        while let Ok(update) = receiver.try_recv() {
            match update {
                DownloadUpdate::Progress(progress) => {
                    self.status = progress.message.clone();
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
                            self.status = format!(
                                "Ready · downloaded, verified, and inspected {} · discovery remains open",
                                report.path.display()
                            );
                            self.image = Some(report.clone());
                            self.advanced = false;
                        }
                        DownloadCompletion::Cancelled => {
                            self.status = "Download cancelled • temporary data cleaned up".into();
                        }
                        DownloadCompletion::Failed(error) => {
                            self.status = format!("Download stopped · {error}")
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
                self.status = "Download paused • press p to resume or x to cancel".into();
            }
            Ok(Some(OperationState::Running)) => self.status = "Download resumed".into(),
            Ok(Some(OperationState::Cancelled) | None) => {}
            Err(error) => self.status = error.to_string(),
        }
    }

    fn cancel_download(&mut self) {
        if self.download_session.cancel() {
            self.status = "Cancelling download safely • cleaning temporary data…".into();
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
                    self.status = "Search applied • scroll for more matching results".into();
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
            self.status = "Type to search • Enter applies • Esc leaves search".into();
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
                self.status = format!("Showing images compatible with {}", device.name);
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
        let query = self.catalog_query.to_lowercase();
        self.distributions
            .iter()
            .enumerate()
            .filter(|(_, distribution)| {
                query.is_empty()
                    || distribution.name.to_lowercase().contains(&query)
                    || distribution.slug.to_lowercase().contains(&query)
                    || distribution
                        .based_on
                        .as_deref()
                        .is_some_and(|value| value.to_lowercase().contains(&query))
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
        if let Some(index) = self.filtered_distribution_indices().first() {
            self.catalog_selected = *index;
        }
        if let Some(index) = self.compatible_pi_image_indices().first() {
            self.pi_image_selected = *index;
        }
    }

    fn choose_image(&mut self) {
        if self.image_loading {
            self.status = "Image inspection is already running".into();
            return;
        }
        let mut dialog = rfd::FileDialog::new().add_filter(
            "Boot images",
            &[
                "iso", "img", "raw", "xz", "gz", "gzip", "zst", "zstd", "bz2", "bzip2",
            ],
        );
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        let Some(path) = dialog.pick_file() else {
            self.status = "Image selection cancelled".into();
            return;
        };
        self.inspect_image_path(path);
    }

    fn inspect_image_path(&mut self, path: PathBuf) {
        if self.image_loading {
            self.status = "Image inspection is already running".into();
            return;
        }
        self.image_loading = true;
        self.status = "Inspecting image • compressed sources are measured after expansion…".into();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = Bootable::native()
                .inspect_image(&path)
                .map(|report| (report, path))
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
        self.image_receiver = Some(receiver);
    }

    fn poll_image(&mut self) {
        let Some(receiver) = self.image_receiver.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok((image, path))) => {
                self.image_loading = false;
                self.status = format!("Recognized {}", image.kind);
                self.browse_directory = path.parent().map(PathBuf::from);
                self.image = Some(image);
                self.advanced = false;
            }
            Ok(Err(error)) => {
                self.image_loading = false;
                self.image = None;
                self.advanced = false;
                self.status = error;
            }
            Err(mpsc::TryRecvError::Empty) => self.image_receiver = Some(receiver),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.image_loading = false;
                self.status = "Image inspection stopped unexpectedly".into();
            }
        }
    }

    fn choose_folder(&mut self) {
        let mut dialog = rfd::FileDialog::new();
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        if let Some(directory) = dialog.pick_folder() {
            self.status = format!("Image browser folder: {}", directory.display());
            self.browse_directory = Some(directory);
        } else {
            self.status = "Folder selection cancelled".into();
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
            self.status = "No eligible removable drive is available".into();
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
        self.status =
            "Target selected · confirm the physical drive before reviewing the erase plan".into();
    }

    fn move_workspace_focus(&mut self, backwards: bool) {
        self.workspace_focus = if backwards {
            self.workspace_focus.previous(self.image.is_some())
        } else {
            self.workspace_focus.next(self.image.is_some())
        };
        self.status = match self.workspace_focus {
            WorkspaceFocus::Source => "Source · choose or change the image",
            WorkspaceFocus::Target => "Target · choose an eligible removable drive",
            WorkspaceFocus::Setup => "Setup options · configure image-specific choices",
            WorkspaceFocus::Review => "Review & write · inspect the plan before erasure",
            WorkspaceFocus::Discover => "Discover images · browse trusted catalogs",
            WorkspaceFocus::Refresh => "Refresh drives · rescan removable media",
        }
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
                self.status =
                    "Drive refresh is paused while writing • do not unplug the target".into();
            }
            return;
        }
        match self.engine.discover_devices() {
            Ok(devices) => {
                if devices == self.devices {
                    if manual {
                        self.status = "Drive list is up to date • automatic detection is on".into();
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
                self.status = device_change_message(added, removed);
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    fn preview(&mut self) {
        let Some(image) = self.image.clone() else {
            self.status = "Start with --image /path/to/image.iso to create a plan".into();
            return;
        };
        let Some(target) = self
            .selected
            .and_then(|index| self.devices.get(index))
            .cloned()
        else {
            self.status = "No target device is selected".into();
            return;
        };
        match self
            .engine
            .plan_with_options(image, target, self.options.clone())
        {
            Ok(plan) => {
                self.catalog_open = false;
                self.status = "Reviewing the write plan • nothing has been written".into();
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
            self.status = "Writing is active • do not close the app or unplug the target".into();
            return;
        }
        self.status = self.review_readiness().guidance().into();
    }

    fn open_write_confirmation(&mut self) {
        if self.write_session.open_confirmation() {
            self.status = "Review the target changes and consequences before writing".into();
        }
    }

    fn close_write_confirmation(&mut self) {
        self.write_session.close_confirmation();
        self.status = "Write cancelled before erasure • the target is unchanged".into();
    }

    fn start_write(&mut self) {
        let launch = match self.write_session.begin() {
            Ok(launch) => launch,
            Err(message) => {
                self.status = message.into();
                return;
            }
        };
        self.status = "Write started • do not unplug the target".into();

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
                    self.status = self.write_session.finish(completion);
                }
            }
        }
        if !finished {
            self.write_receiver = Some(receiver);
        }
    }

    fn cancel_write(&mut self) {
        if self.write_session.cancel() {
            self.status =
                "Stopping safely • flushing completed writes; media will remain incomplete".into();
        }
    }

    fn toggle_windows_requirements(&mut self) {
        let Some(image) = &self.image else {
            self.status = "Choose a Windows image before changing Windows options".into();
            return;
        };
        if !matches!(
            image.kind,
            bootable_core::ImageKind::WindowsInstaller { .. }
        ) {
            self.status = "Windows setup options apply only to Windows installer images".into();
            return;
        }
        let enabled = &mut self.options.windows.bypass_hardware_requirements;
        *enabled = !*enabled;
        self.status = if *enabled {
            "Windows 11 TPM, Secure Boot, and RAM checks will be bypassed".into()
        } else {
            "Windows 11 hardware checks use Microsoft defaults".into()
        };
    }

    fn toggle_windows_offline_account(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        let enabled = &mut self.options.windows.allow_offline_account;
        *enabled = !*enabled;
        self.status = if *enabled {
            "Windows OOBE will expose the offline/local-account path".into()
        } else {
            "Windows OOBE will use its standard account flow".into()
        };
    }

    fn toggle_windows_privacy(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        let enabled = &mut self.options.windows.minimize_data_collection;
        *enabled = !*enabled;
        self.status = if *enabled {
            "Windows OOBE will use privacy-focused defaults".into()
        } else {
            "Windows OOBE privacy questions will remain at their defaults".into()
        };
    }

    fn toggle_windows_bitlocker(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        let enabled = &mut self.options.windows.disable_bitlocker;
        *enabled = !*enabled;
        self.status = if *enabled {
            "Automatic Windows device encryption will be disabled".into()
        } else {
            "Windows may automatically enable device encryption".into()
        };
    }

    fn toggle_windows_named_account(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        if self.options.windows.local_account.is_some() {
            self.options.windows.local_account = None;
            self.status = "Automatic local-account creation disabled".into();
        } else {
            let account = bootable_core::suggested_account_name().unwrap_or_else(|| "User".into());
            self.options.windows.local_account = Some(account.clone());
            self.options.windows.allow_offline_account = true;
            self.status = format!("Windows will create local administrator account `{account}`");
        }
    }

    fn toggle_windows_regional(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        if self.options.windows.regional.is_some() {
            self.options.windows.regional = None;
            self.status = "Windows Setup will ask for regional options".into();
        } else {
            let regional = bootable_core::host_regional_options();
            self.status = format!(
                "Windows will use locale {} and time zone {}",
                regional.user_locale, regional.time_zone
            );
            self.options.windows.regional = Some(regional);
        }
    }

    fn toggle_windows_qol(&mut self) {
        if self.windows_options_available() {
            self.options.windows.quality_of_life = !self.options.windows.quality_of_life;
            self.status = "Windows QoL policy selection updated".into();
        }
    }

    fn toggle_windows_ca_2023(&mut self) {
        if self.windows_options_available() {
            self.options.windows.use_windows_ca_2023 = !self.options.windows.use_windows_ca_2023;
            self.status = "CA 2023 boot media requires updated Secure Boot certificates".into();
        }
    }

    fn toggle_windows_skusi_policy(&mut self) {
        if self.windows_options_available() {
            self.options.windows.apply_skusi_policy = !self.options.windows.apply_skusi_policy;
            self.status = "SkuSiPolicy.p7b selection updated".into();
        }
    }

    fn toggle_windows_s_mode(&mut self) {
        if self.windows_options_available() {
            self.options.windows.force_s_mode = !self.options.windows.force_s_mode;
            self.status =
                "S Mode may remain enforced after reinstall; review the plan carefully".into();
        }
    }

    fn cycle_windows_partition_scheme(&mut self) {
        if !self.windows_options_available() {
            return;
        }
        self.options.windows_partition_scheme = match self.options.windows_partition_scheme {
            bootable_core::WindowsPartitionScheme::Gpt => {
                bootable_core::WindowsPartitionScheme::Mbr
            }
            bootable_core::WindowsPartitionScheme::Mbr => {
                bootable_core::WindowsPartitionScheme::Gpt
            }
        };
        self.status = format!(
            "Windows partition scheme: {} · target firmware: UEFI",
            self.options.windows_partition_scheme
        );
    }

    fn windows_options_available(&mut self) -> bool {
        let available = self.image.as_ref().is_some_and(|image| {
            matches!(
                image.kind,
                bootable_core::ImageKind::WindowsInstaller { .. }
            )
        });
        if !available {
            self.status = "Choose a Windows installer image before changing Windows options".into();
        }
        available
    }

    fn toggle_advanced(&mut self) {
        if self.image.is_none() {
            self.advanced = false;
            self.status = "Choose or download an image before opening media options".into();
            return;
        }
        self.advanced = !self.advanced;
        self.status = if self.advanced {
            "Advanced options expanded • every option is included in the reviewed plan".into()
        } else {
            "Advanced options collapsed • configured values remain active".into()
        };
    }

    fn cycle_checksum_algorithm(&mut self) {
        self.checksum_algorithm = self.checksum_algorithm.next();
        self.status = format!("Checksum algorithm: {}", self.checksum_algorithm);
    }

    fn cycle_bad_blocks(&mut self) {
        self.options.bad_block_check = self.options.bad_block_check.next();
        self.status = match self.options.bad_block_check {
            BadBlockCheck::Disabled => "Destructive bad-block check disabled".into(),
            mode => format!(
                "Bad-block check: {} destructive pattern(s) before writing",
                mode.passes()
            ),
        };
    }

    fn checksum(&mut self) {
        let Some(image) = &self.image else {
            self.status = "Choose an image before computing its checksum".into();
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
            self.status = "Choose a removable drive to back up".into();
            return;
        };
        let mut dialog = rfd::FileDialog::new()
            .add_filter("Raw drive image", &["img", "raw", "dd"])
            .set_file_name("bootable-backup.img");
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        let Some(destination) = dialog.save_file() else {
            self.status = "Drive backup cancelled".into();
            return;
        };
        self.status = format!("Backing up {}…", device.display_name());
        let mut latest = self.status.clone();
        let result = self
            .engine
            .backup_device(device.id.as_str(), &destination, |progress| {
                latest = progress.message;
            });
        self.status = match result {
            Ok(()) => format!("Drive image saved to {}", destination.display()),
            Err(error) => format!("{error} • last step: {latest}"),
        };
    }

    fn handle_write_flow_click(&mut self, point: (u16, u16)) -> Option<bool> {
        if contains(self.hit_regions.download_pause, point) {
            self.toggle_download_pause();
        } else if contains(self.hit_regions.download_cancel, point) {
            self.cancel_download();
        } else if contains(self.hit_regions.open_image, point) {
            self.choose_image();
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
                self.status =
                    "Target selected · confirm the physical drive before reviewing the erase plan"
                        .into();
            } else {
                self.status = "That drive is blocked and cannot be selected".into();
            }
        }
        Some(false)
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
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
                if contains(self.hit_regions.review_back, point) {
                    self.close_review();
                } else if contains(self.hit_regions.review_write, point) {
                    if self.write_session.active() {
                        self.cancel_write();
                    } else {
                        self.open_write_confirmation();
                    }
                } else if contains(self.hit_regions.quit, point) {
                    if self.write_session.active() {
                        self.status =
                            "Writing is active • do not close the app or unplug the target".into();
                    } else {
                        return true;
                    }
                }
            }
            return false;
        }
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            let point = (mouse.column, mouse.row);
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
                        self.status = "Type to search • Enter applies • Esc leaves search".into();
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
                        self.status =
                            "Raspberry Pi image selected • download will be verified".into();
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
                        self.status = "ISO selected • choose Download & use ISO".into();
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
                                        "Acknowledge the consequences before confirming the write"
                                            .into();
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
                                app.status = "Writing is active • press x to stop safely; do not unplug the target".into();
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
                        KeyCode::Char('?') => {
                            app.status = "Keyboard: Tab / Shift+Tab moves focus · Enter activates · arrows choose a target · o image · g discover · a setup · p review · r refresh · q quit".into();
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
    let bad_blocks = match app.options.bad_block_check.passes() {
        0 => "Bad blocks off".into(),
        passes => format!("Bad blocks {passes}x"),
    };
    render_button(
        frame,
        area,
        &format!("+  Setup options · Verification on · {bad_blocks}"),
        app.workspace_focus == WorkspaceFocus::Setup,
    );
    app.hit_regions.advanced = Some(area);
}

fn draw_collapsed_discovery(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    render_button(
        frame,
        area,
        "+  Discover images · All · Arch · Debian · Omarchy · Windows · Raspberry Pi",
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
    let steps = plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let marker = if step.destructive {
                "ERASES DATA"
            } else {
                "safe"
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
        constraints.push(Constraint::Length(6));
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
    frame.render_widget(
        Paragraph::new(brand_lockup(
            area.width >= 60,
            "Review write plan",
            if app.write_session.active() {
                "Writing and verification are active • do not unplug the target."
            } else {
                "Nothing is written until the consequences are reviewed and acknowledged."
            },
        ))
        .style(Style::default().bg(BG)),
        rows[row],
    );
    row += 1;
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("SOURCE  ", Style::default().fg(MUTED)),
                Span::styled(source, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("TARGET  ", Style::default().fg(MUTED)),
                Span::styled(target, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("METHOD  ", Style::default().fg(MUTED)),
                Span::styled(method, Style::default().fg(ACCENT)),
            ]),
        ])
        .wrap(Wrap { trim: true })
        .block(panel_block(" Plan summary ")),
        rows[row],
    );
    row += 1;
    if show_steps {
        frame.render_widget(
            List::new(steps).block(panel_block(" Ordered operations ")),
            rows[row],
        );
    }
    row += 1;
    if show_confirmation {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    "All existing data and partitions on the selected target will be erased.",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Line::styled(
                    "Open the confirmation to review changes, consequences, and the physical target.",
                    Style::default().fg(MUTED),
                ),
            ])
            .wrap(Wrap { trim: true })
            .block(panel_block(" Permanent changes ")),
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
        let progress_title = format!(" {} · {} ", progress.phase, progress.message);
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
        let (title, message, color) = match completion {
            WriteCompletion::Succeeded => (
                " Write complete ",
                "Image written and verified. The removable drive can now be safely removed."
                    .to_string(),
                ACCENT,
            ),
            WriteCompletion::AuthenticationDenied => (
                " Write cancelled before erasure ",
                "Administrator authentication was cancelled or denied.".into(),
                Color::Yellow,
            ),
            WriteCompletion::Cancelled => (
                " Write stopped safely ",
                "The media is incomplete and must be rewritten before use.".into(),
                Color::LightRed,
            ),
            WriteCompletion::Failed(error) => (" Write failed ", error.clone(), Color::LightRed),
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::default().fg(color))
                .wrap(Wrap { trim: true })
                .block(panel_block(title)),
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
    if app.write_session.active() {
        render_disabled_button(frame, actions[0], "←  Back locked");
    } else {
        render_button(frame, actions[0], "←  Back to selection", false);
    }
    let write_enabled = !write_succeeded;
    let write_label = if app.write_session.active() {
        "■  Stop safely"
    } else if write_succeeded {
        "✓  Written & verified"
    } else if app.write_session.completion().is_some() {
        "!  Review & retry"
    } else {
        "!  Review consequences"
    };
    if write_enabled {
        render_button(frame, actions[1], write_label, true);
    } else {
        render_disabled_button(frame, actions[1], write_label);
    }
    if app.write_session.active() {
        render_disabled_button(frame, actions[2], "×  Quit locked");
    } else {
        render_button(frame, actions[2], "×  Quit", false);
    }
    app.hit_regions.review_back = (!app.write_session.active()).then_some(actions[0]);
    app.hit_regions.review_write = write_enabled.then_some(actions[1]);
    app.hit_regions.quit = (!app.write_session.active()).then_some(actions[2]);
    if app.write_session.confirmation_open() {
        draw_write_confirmation_modal(frame, app, area);
    }
}

fn draw_write_confirmation_modal(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
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
    let compact = modal.height < 28;
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
                "ERASES DATA"
            } else {
                "verifies"
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

    frame.render_widget(Clear, modal);
    frame.render_widget(
        Block::default()
            .title(" Confirm permanent changes · PERMANENT ")
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
    let inner = modal.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    let rows = if compact {
        Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .spacing(1)
        .split(inner)
    } else {
        Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(7),
            Constraint::Length(7),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .spacing(1)
        .split(inner)
    };
    frame.render_widget(
        Paragraph::new(target)
            .wrap(Wrap { trim: true })
            .block(panel_block(" Physical target · check carefully ")),
        rows[0],
    );
    if compact {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    "• All existing files and partitions on this drive will be permanently erased.",
                    Style::default().fg(Color::LightRed),
                ),
                Line::styled(
                    "• Choosing the wrong physical drive destroys its data.",
                    Style::default().fg(Color::Yellow),
                ),
                Line::styled(
                    "• Do not close, power off, or unplug until verification finishes.",
                    Style::default().fg(Color::Yellow),
                ),
            ])
            .wrap(Wrap { trim: true })
            .block(panel_block(" Changes and consequences ")),
            rows[1],
        );
    } else {
        frame.render_widget(
            List::new(changes).block(panel_block(" Changes to this drive ")),
            rows[1],
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    "• Every existing file and partition on this physical drive becomes unrecoverable without a separate backup.",
                    Style::default().fg(Color::LightRed),
                ),
                Line::styled(
                    "• Selecting the wrong drive destroys the data on that drive.",
                    Style::default().fg(Color::Yellow),
                ),
                Line::styled(
                    "• Power loss, closing, or unplugging can leave incomplete and unbootable media.",
                    Style::default().fg(Color::Yellow),
                ),
                Line::styled(
                    "• Bootable rechecks target identity before erasure and verifies the result afterward.",
                    Style::default().fg(MUTED),
                ),
            ])
            .wrap(Wrap { trim: true })
            .block(panel_block(" Consequences ")),
            rows[2],
        );
    }
    let acknowledgment_row = if compact { rows[2] } else { rows[3] };
    let actions_row = if compact { rows[3] } else { rows[4] };
    let acknowledgment = if app.write_session.acknowledged() {
        "■ I checked the physical target and understand its existing data will be permanently erased."
    } else {
        "□ I checked the physical target and understand its existing data will be permanently erased."
    };
    frame.render_widget(
        Paragraph::new(acknowledgment)
            .style(Style::default().fg(if app.write_session.acknowledged() {
                ACCENT
            } else {
                Color::White
            }))
            .wrap(Wrap { trim: true })
            .block(panel_block(" Space/click to acknowledge ")),
        acknowledgment_row,
    );
    let actions = Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .spacing(1)
        .split(actions_row);
    render_button(frame, actions[0], "←  Cancel · target unchanged", false);
    let confirm_ready = app.write_session.can_confirm();
    if confirm_ready {
        render_danger_button(frame, actions[1], "!  Confirm erase & write");
    } else {
        render_disabled_button(frame, actions[1], "□  Acknowledge first");
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
    let action_count = if app.image.is_some() { 4 } else { 3 };
    let action_width = if wide {
        if action_count == 4 { 64 } else { 48 }
    } else {
        area.width.saturating_sub(13)
    };
    let columns = Layout::horizontal([
        Constraint::Min(if wide { 24 } else { 12 }),
        Constraint::Length(action_width),
    ])
    .spacing(1)
    .split(area);
    frame.render_widget(
        Paragraph::new(brand_lockup(
            wide,
            "Create boot media",
            "One deliberate path from image to removable drive.",
        ))
        .style(Style::default().bg(BG)),
        columns[0],
    );
    let actions = Layout::horizontal(vec![
        Constraint::Ratio(1, action_count as u32);
        action_count
    ])
    .spacing(1)
    .split(columns[1]);
    render_button(
        frame,
        actions[0],
        if wide { "⇩ Downloads" } else { "⇩ Jobs" },
        app.downloads_open,
    );
    render_button(
        frame,
        actions[1],
        if app.catalog_open {
            if wide { "× Catalog" } else { "× Cat" }
        } else {
            if wide { "⌄ Discover" } else { "⌄ Find" }
        },
        app.workspace_focus == WorkspaceFocus::Discover,
    );
    if app.image.is_some() {
        render_button(
            frame,
            actions[2],
            if app.advanced {
                if wide { "⚙ Hide options" } else { "⚙ Hide" }
            } else {
                if wide {
                    "⚙ Setup options"
                } else {
                    "⚙ Setup"
                }
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
        if wide { "↻ Refresh" } else { "↻ USB" },
        app.workspace_focus == WorkspaceFocus::Refresh,
    );
    app.hit_regions.downloads = Some(actions[0]);
    app.hit_regions.discover = Some(actions[1]);
    app.hit_regions.refresh = Some(actions[refresh_index]);
    draw_workspace_steps(frame, app, header_rows[1]);
}

fn draw_workspace_steps(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let progress = workspace_progress(
        app.image.as_ref(),
        app.selected.and_then(|index| app.devices.get(index)),
    );
    let line = Line::from(vec![
        step_span("1 Source", progress.source),
        Span::styled("  ─────  ", Style::default().fg(BORDER)),
        step_span("2 Target", progress.target),
        Span::styled("  ─────  ", Style::default().fg(BORDER)),
        step_span("3 Review & write", progress.review),
    ]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn step_span(label: &'static str, state: WorkspaceStepState) -> Span<'static> {
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

fn brand_lockup<'a>(wide: bool, context: &'a str, subtitle: &'a str) -> Vec<Line<'a>> {
    if !wide {
        return vec![Line::from(vec![
            Span::styled(
                " USB♨  BOOTABLE α ",
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {context}"), Style::default().fg(Color::White)),
        ])];
    }
    vec![
        Line::from(vec![
            Span::styled(
                "┌┬┬┐",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  BOOTABLE α",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  ·  {context}"), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("╰♨─╯", Style::default().fg(ACCENT)),
            Span::styled(format!("  {subtitle}"), Style::default().fg(MUTED)),
        ]),
    ]
}

fn draw_source(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let source = app
        .image
        .as_ref()
        .map(|image| {
            format!(
                "{}\n{} • {}  ·  ✓ Inspected",
                image.path.display(),
                image.kind,
                format_bytes(image.size)
            )
        })
        .unwrap_or_else(|| {
            "ISO, IMG, RAW, or compressed disk image\nInspected before writing".into()
        });
    let source_block = focused_panel_block(
        " 1  Source · choose an image ",
        app.workspace_focus == WorkspaceFocus::Source,
    );
    let source_inner = source_block.inner(area);
    frame.render_widget(source_block, area);
    let source_columns = Layout::horizontal([Constraint::Min(16), Constraint::Length(14)])
        .spacing(1)
        .split(source_inner);
    frame.render_widget(
        Paragraph::new(source)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: true }),
        source_columns[0],
    );
    let button_area = centered_button_area(source_columns[1]);
    if app.image_loading {
        render_disabled_button(frame, button_area, "…  Inspecting");
        app.hit_regions.open_image = None;
    } else {
        render_button(
            frame,
            button_area,
            if app.image.is_some() {
                "▣  Change"
            } else {
                "▣  Browse"
            },
            true,
        );
        app.hit_regions.open_image = Some(button_area);
    }
}

fn draw_download_manager(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let rows = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(4),
        Constraint::Length(3),
    ])
    .spacing(1)
    .split(area);
    let items = if app.download_session.jobs().is_empty() {
        vec![
            ListItem::new("No managed downloads yet · choose an image from Discover to begin")
                .style(Style::default().fg(MUTED)),
        ]
    } else {
        app.download_session
            .jobs()
            .iter()
            .map(|job| {
                let progress = job
                    .progress_ratio()
                    .map(|ratio| format!(" · {:>5.1}%", ratio * 100.))
                    .unwrap_or_default();
                ListItem::new(format!(
                    "{:<11} {:<28} {}{}",
                    job.status,
                    truncate_middle(&job.label, 28),
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
    frame.render_stateful_widget(
        List::new(items)
            .block(panel_block(
                " Downloads · persistent history · ↑/↓ select · m closes ",
            ))
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
        || "Interrupted transfers retain only owned partial files; explicit cancellation removes them.".into(),
        |job| {
            format!(
                "{} · {}\n{}",
                job.kind,
                job.error.as_deref().unwrap_or(&job.message),
                job.destination.display()
            )
        },
    );
    frame.render_widget(
        Paragraph::new(details)
            .style(Style::default().fg(MUTED))
            .wrap(Wrap { trim: true })
            .block(panel_block(" Selected download ")),
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
    if can_retry {
        render_button(frame, actions[0], "↻  Retry / resume", true);
    } else {
        render_disabled_button(frame, actions[0], "↻  Retry / resume");
    }
    if can_use {
        render_button(frame, actions[1], "✓  Use image", true);
    } else {
        render_disabled_button(frame, actions[1], "✓  Use image");
    }
    if can_remove {
        render_button(frame, actions[2], "×  Remove entry", false);
    } else {
        render_disabled_button(frame, actions[2], "×  Remove entry");
    }
    app.hit_regions.download_rows = catalog_row_regions(rows[0], app.download_session.jobs().len());
    app.hit_regions.download_retry = can_retry.then_some(actions[0]);
    app.hit_regions.download_use = can_use.then_some(actions[1]);
    app.hit_regions.download_remove = can_remove.then_some(actions[2]);
}

fn truncate_middle(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.into();
    }
    let side = limit.saturating_sub(1) / 2;
    let start = value.chars().take(side).collect::<String>();
    let end = value
        .chars()
        .rev()
        .take(side)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{start}…{end}")
}

fn draw_catalog(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    frame.render_widget(Clear, area);
    let block = panel_block(" Discover bootable images ");
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
        "1  All",
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
    let search_value = if app.discovery_session.quick_access() == QuickAccess::Windows {
        "Windows installer workflow · select an ISO to unlock every setup checkbox".into()
    } else if app.catalog_query.is_empty() {
        "Search distributions…  / to type".into()
    } else {
        format!(
            "{}{}",
            app.catalog_query,
            if app.catalog_searching { "▏" } else { "" }
        )
    };
    frame.render_widget(
        Paragraph::new(search_value)
            .style(search_style)
            .block(panel_block(" Search ")),
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
    render_button(frame, actions[0], "↻  Retry", false);
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
        if app.discovery_session.quick_access() == QuickAccess::Windows {
            "▣  Choose Windows ISO"
        } else if app.discovery_session.source() == DiscoverySource::RaspberryPi {
            "⇩  Download, verify & use"
        } else if open_page_fallback {
            "↗  Open DistroWatch download page  [b]"
        } else {
            "⇩  Download & use ISO"
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
    let windows_image = app.image.as_ref().is_some_and(|image| {
        matches!(
            image.kind,
            bootable_core::ImageKind::WindowsInstaller { .. }
        )
    });
    let columns = windows_option_columns(area.width);
    let option_rows = 11_usize.div_ceil(columns);
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
                if windows_image {
                    "Windows ISO ready · setup choices unlocked"
                } else {
                    "Choose a Windows ISO to unlock setup choices"
                },
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
    let options = grid_areas(rows[1], columns, 11);
    render_checkbox(
        frame,
        options[0],
        "Hardware bypass",
        app.options.windows.bypass_hardware_requirements,
    );
    render_checkbox(
        frame,
        options[1],
        "Offline account",
        app.options.windows.allow_offline_account,
    );
    render_checkbox(
        frame,
        options[2],
        "Privacy defaults",
        app.options.windows.minimize_data_collection,
    );
    render_checkbox(
        frame,
        options[3],
        "Disable BitLocker",
        app.options.windows.disable_bitlocker,
    );
    render_checkbox(
        frame,
        options[4],
        "Named account",
        app.options.windows.local_account.is_some(),
    );
    render_checkbox(
        frame,
        options[5],
        "Host region",
        app.options.windows.regional.is_some(),
    );
    render_checkbox(
        frame,
        options[6],
        "QoL policies",
        app.options.windows.quality_of_life,
    );
    render_checkbox(
        frame,
        options[7],
        "CA 2023",
        app.options.windows.use_windows_ca_2023,
    );
    render_checkbox(
        frame,
        options[8],
        "SkuSiPolicy",
        app.options.windows.apply_skusi_policy,
    );
    render_checkbox(
        frame,
        options[9],
        "Force S Mode",
        app.options.windows.force_s_mode,
    );
    render_button(
        frame,
        options[10],
        &format!("Scheme: {}", app.options.windows_partition_scheme),
        true,
    );
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
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                "✓ Standard install · GPT/UEFI/FAT32 · split WIM · requirements · account · region · privacy · BitLocker",
                Style::default().fg(ACCENT),
            ),
            Line::styled(
                "✓ QoL · CA 2023 · SkuSiPolicy · S Mode · checksums · bad blocks · verified write · safety gates",
                Style::default().fg(ACCENT),
            ),
            Line::styled(
                "○ Windows To Go/internal-disk isolation · legacy BIOS + NTFS/UEFI:NTFS · silent install",
                Style::default().fg(Color::Yellow),
            ),
            Line::styled(
                "Unavailable items are not clickable. Existing autounattend.xml files are never overwritten.",
                Style::default().fg(MUTED),
            ),
        ])
        .wrap(Wrap { trim: true })
        .block(panel_block(" Complete Rufus 4.15 Windows coverage ")),
        rows[2],
    );
}

fn draw_distrowatch_catalog(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
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
            "No matching distributions".into()
        } else if !app.catalog_query.is_empty() {
            app.discovery_session
                .state(CatalogFacet::Directory)
                .short_label("search catalog")
        } else {
            current_distribution_state(app).short_label("distributions")
        };
        vec![ListItem::new(message).style(Style::default().fg(MUTED))]
    } else {
        matching_indices
            .iter()
            .filter_map(|index| app.distributions.get(*index))
            .map(|distribution| {
                ListItem::new(if distribution.rank == 0 {
                    format!(" ·  {:<26} DistroWatch", distribution.name)
                } else {
                    format!(
                        "{:>2}  {:<20} {:>5}",
                        distribution.rank, distribution.name, distribution.hits_per_day
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
                    .short_label("ISO releases"),
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
                    .unwrap_or_else(|| "HTTPS only".into());
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
    frame.render_stateful_widget(
        List::new(distributions)
            .block(panel_block(" Popular distributions "))
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
            .short_label("distribution profile")
    } else {
        app.selected_details.as_ref().map_or_else(
            || "Choose a distribution to load its profile and ISO files".into(),
            |details| {
            format!(
                "{}  ·  {}  ·  {}\nBased on: {}  ·  Origin: {}\nArchitecture: {}\nDesktop: {}\n{}\nLogo: {}\nScreenshot: {}",
                details.name,
                details.os_type.as_deref().unwrap_or("Unknown OS"),
                details.status.as_deref().unwrap_or("Unknown status"),
                details.based_on.as_deref().unwrap_or("Independent"),
                details.origin.as_deref().unwrap_or("Unknown"),
                compact_text_list(&details.architectures, 4),
                compact_text_list(&details.desktops, 4),
                details.description.as_deref().unwrap_or("No description"),
                details.logo_url.as_deref().unwrap_or("Not listed"),
                details.screenshot_url.as_deref().unwrap_or("Not listed")
            )
            },
        )
    };
    draw_catalog_artwork_panel(
        frame,
        app,
        right[0],
        " Distribution profile · artwork ",
        profile,
    );
    frame.render_stateful_widget(
        List::new(releases)
            .block(panel_block(" Direct ISO files "))
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
                        .short_label("Raspberry Pi boards"),
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
        let message = if app.pi_catalog.is_some() {
            "No compatible images".into()
        } else {
            app.discovery_session
                .state(CatalogFacet::RaspberryPi)
                .short_label("Raspberry Pi images")
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
    frame.render_stateful_widget(
        List::new(devices)
            .block(panel_block(" Raspberry Pi board "))
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
        || "Choose a board and image. Official checksums are verified before the image is used.".into(),
        |image| {
            format!(
                "{}\n{}\nReleased {}  ·  Download {}  ·  Expanded {}\nCategory: {}\nArchive: {}  ·  SHA-256: {}",
                image.name,
                image.description.as_deref().unwrap_or("No description"),
                image.release_date.as_deref().unwrap_or("unknown"),
                image.download_size.map(format_bytes).unwrap_or_default(),
                image.extracted_size.map(format_bytes).unwrap_or_default(),
                image.category.as_deref().unwrap_or("Raspberry Pi image"),
                image.archive_name,
                if image.extracted_sha256.is_some() { "available" } else { "not listed" }
            )
        },
    );
    draw_catalog_artwork_panel(
        frame,
        app,
        right[0],
        " Image details · official Imager feed ",
        details,
    );
    let mut image_state =
        ListState::default().with_selected((!visible_images.is_empty()).then_some(image_position));
    frame.render_stateful_widget(
        List::new(image_items)
            .block(panel_block(" Compatible boot images "))
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
    title: &'static str,
    text: String,
) {
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
                if app.artwork_key.is_some() {
                    "Loading artwork…"
                } else {
                    "No artwork"
                }
            },
            |_| "Artwork unavailable",
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

fn compact_text_list(values: &[String], limit: usize) -> String {
    if values.is_empty() {
        return "Not listed".into();
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
    let block = focused_panel_block(
        " Setup options ",
        app.workspace_focus == WorkspaceFocus::Setup,
    );
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
            format!("Windows installer options  ·  checkboxes  ·  {selected} selected")
        } else {
            "Linux / Unix boot media  ·  active features".into()
        })
        .style(Style::default().fg(MUTED)),
        rows[0],
    );
    let windows = grid_areas(rows[1], if compact { 2 } else { 4 }, 4);
    let tools = grid_areas(rows[2], if compact { 3 } else { 5 }, 5);

    if windows_image {
        render_checkbox(
            frame,
            windows[0],
            "Hardware bypass",
            app.options.windows.bypass_hardware_requirements,
        );
        render_checkbox(
            frame,
            windows[1],
            "Local account",
            app.options.windows.allow_offline_account,
        );
        render_checkbox(
            frame,
            windows[2],
            "Privacy defaults",
            app.options.windows.minimize_data_collection,
        );
        render_checkbox(
            frame,
            windows[3],
            "Disable BitLocker",
            app.options.windows.disable_bitlocker,
        );
        app.hit_regions.windows_options = Some(windows[0]);
        app.hit_regions.windows_offline = Some(windows[1]);
        app.hit_regions.windows_privacy = Some(windows[2]);
        app.hit_regions.windows_bitlocker = Some(windows[3]);
    } else {
        render_checkbox(frame, windows[0], "Full disk layout", true);
        render_checkbox(frame, windows[1], "Boot records", true);
        render_checkbox(frame, windows[2], "Byte verification", true);
        render_checkbox(frame, windows[3], "Safe unmount", true);
        app.hit_regions.windows_options = None;
        app.hit_regions.windows_offline = None;
        app.hit_regions.windows_privacy = None;
        app.hit_regions.windows_bitlocker = None;
    }

    let bad_blocks = match app.options.bad_block_check.passes() {
        0 => "Bad blocks: off".into(),
        passes => format!("Bad blocks: {passes}x"),
    };
    render_button(frame, tools[0], &format!("◌  {bad_blocks}"), false);
    render_button(
        frame,
        tools[1],
        &format!("#  {}", app.checksum_algorithm),
        false,
    );
    render_button(frame, tools[2], "✓  Verify image", false);
    render_button(frame, tools[3], "▢  Image folder", false);
    render_button(frame, tools[4], "⇩  Back up drive", false);

    app.hit_regions.bad_blocks = Some(tools[0]);
    app.hit_regions.checksum_algorithm = Some(tools[1]);
    app.hit_regions.checksum = Some(tools[2]);
    app.hit_regions.choose_folder = Some(tools[3]);
    app.hit_regions.backup = Some(tools[4]);
}

fn draw_targets(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let items = if app.devices.is_empty() {
        vec![ListItem::new("No removable drives detected").style(Style::default().fg(MUTED))]
    } else {
        app.devices
            .iter()
            .map(|device| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:<12}", device.path.display()),
                        Style::default().fg(if device.is_eligible_target() {
                            ACCENT
                        } else {
                            Color::LightRed
                        }),
                    ),
                    Span::raw(format!(
                        " {:>9}  {}  ·  {}",
                        format_bytes(device.capacity),
                        device.display_name(),
                        target_eligibility_label(device)
                    )),
                ]))
            })
            .collect::<Vec<_>>()
    };
    let target_block = focused_panel_block(
        " 2  Target · removable media ",
        app.workspace_focus == WorkspaceFocus::Target,
    );
    let target_inner = target_block.inner(area);
    frame.render_widget(target_block, area);
    let target_rows =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(target_inner);
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
        target_rows[0],
        &mut state,
    );
    frame.render_widget(
        Paragraph::new("Confirm the physical drive · erasure starts only after review")
            .style(Style::default().fg(MUTED)),
        target_rows[1],
    );
    app.hit_regions.device_rows = (0..app.devices.len())
        .filter_map(|index| {
            let y = target_rows[0].y.saturating_add(index as u16);
            (y < target_rows[0].bottom()).then_some((
                Rect::new(target_rows[0].x, y, target_rows[0].width, 1),
                index,
            ))
        })
        .collect();
}

fn draw_status(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let status_block = focused_panel_block(
        " 3  Review & write ",
        app.workspace_focus == WorkspaceFocus::Review,
    );
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
        Paragraph::new(app.status.as_str())
            .style(Style::default().fg(MUTED))
            .wrap(Wrap { trim: true }),
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
            workspace.status()
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
            if state == OperationState::Paused {
                "▶  Resume"
            } else {
                "Ⅱ  Pause"
            },
            state != OperationState::Cancelled,
        );
        render_button(
            frame,
            actions[1],
            if state == OperationState::Cancelled {
                "Cancelling…"
            } else {
                "×  Cancel download"
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
            "✓ Review"
        } else {
            "✓  Review plan"
        }
    } else {
        readiness.action_label()
    };
    render_button(
        frame,
        actions[0],
        review_label,
        readiness == ReviewReadiness::Ready,
    );
    render_button(
        frame,
        actions[1],
        if compact { "× Quit" } else { "×  Quit" },
        false,
    );
    app.hit_regions.preview = (readiness == ReviewReadiness::Ready).then_some(actions[0]);
    app.hit_regions.quit = Some(actions[1]);
}

fn render_button(frame: &mut ratatui::Frame<'_>, area: Rect, label: &str, primary: bool) {
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
        Paragraph::new(label)
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
        Paragraph::new(label)
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
    if width >= 72 { 10 } else { 18 }
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
        Paragraph::new("┌┬┬┐  BOOTABLE α\n╰♨─╯\n\nResize to at least 44 × 22\nq  Quit")
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

fn device_change_message(added: usize, removed: usize) -> String {
    match (added, removed) {
        (0, 0) => "Drive details changed • list updated automatically".into(),
        (added, 0) => format!("Detected {added} new drive(s) • list updated automatically"),
        (0, removed) => format!("Removed {removed} drive(s) • list updated automatically"),
        (added, removed) => {
            format!("Drive list changed: {added} added, {removed} removed • updated automatically")
        }
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
        assert_eq!(workspace_height(120), 10);
        assert_eq!(workspace_height(70), 18);
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
        let lines = brand_lockup(true, "Create boot media", "Deliberate writing");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].to_string().contains("┌┬┬┐  BOOTABLE α"));
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
