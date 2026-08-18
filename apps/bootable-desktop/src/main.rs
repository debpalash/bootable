use std::borrow::Cow;
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bootable_core::{
    BadBlockCheck, Bootable, CacheMode, CatalogState, ChecksumAlgorithm, Device,
    DistributionBundle, DistributionDetails, DistributionSummary, DownloadJob, DownloadStatus,
    ImageKind, ImageReport, IsoRelease, OperationControl, OperationState, PiCatalog, PiImage,
    Progress, ReviewReadiness, WindowsPartitionScheme, WriteOptions, WritePlan,
    destructive_confirmation_ready, format_bytes, review_readiness,
};
use futures::{
    AsyncReadExt, FutureExt, StreamExt,
    channel::{mpsc, oneshot},
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{
    Disableable, Icon, Root, TitleBar,
    button::*,
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement,
    select::{Select, SelectEvent, SelectState},
};
use gpui_http_client::{AsyncBody, HttpClient, Url, http};

const DEVICE_SCAN_INTERVAL: Duration = Duration::from_secs(1);
const DOWNLOAD_SCAN_INTERVAL: Duration = Duration::from_secs(5);
const BRAND_MARK_SVG: &[u8] = include_bytes!("../../../assets/bootable-mark.svg");
const BRAND_LOGO_SVG: &[u8] = include_bytes!("../../../assets/bootable-logo.svg");
const IMAGE_ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/image.svg");
const USB_ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/usb.svg");
const SETTINGS_ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/settings.svg");
const REFRESH_ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/refresh.svg");
const HASH_ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/hash.svg");
const FOLDER_ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/folder.svg");
const BACKUP_ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/backup.svg");
const REVIEW_ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/review.svg");
const DISCOVER_ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/discover.svg");
const DOWNLOAD_ICON_SVG: &[u8] = include_bytes!("../../../assets/icons/download.svg");
const MAX_PREVIEW_BYTES: u64 = 16 * 1024 * 1024;

const WINDOW_MINIMIZE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M5 12h14"/></svg>"#;
const WINDOW_MAXIMIZE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="5" y="5" width="14" height="14" rx="1"/></svg>"#;
const WINDOW_RESTORE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M8 8V5h11v11h-3"/><rect x="5" y="8" width="11" height="11" rx="1"/></svg>"#;
const WINDOW_CLOSE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg>"#;

struct PreviewHttpClient {
    client: reqwest::blocking::Client,
    user_agent: http::HeaderValue,
}

impl PreviewHttpClient {
    fn new() -> anyhow::Result<Self> {
        let user_agent = http::HeaderValue::from_static("Bootable/0.1 image preview");
        let client = reqwest::blocking::Client::builder()
            .user_agent(user_agent.to_str()?)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(45))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()?;
        Ok(Self { client, user_agent })
    }
}

impl HttpClient for PreviewHttpClient {
    fn type_name(&self) -> &'static str {
        "BootablePreviewHttpClient"
    }

    fn user_agent(&self) -> Option<&http::HeaderValue> {
        Some(&self.user_agent)
    }

    fn send(
        &self,
        request: http::Request<AsyncBody>,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<http::Response<AsyncBody>>> {
        let client = self.client.clone();
        async move {
            let (parts, mut body) = request.into_parts();
            let mut request_body = Vec::new();
            body.read_to_end(&mut request_body).await?;
            let (sender, receiver) = oneshot::channel();
            std::thread::Builder::new()
                .name("bootable-image-preview".into())
                .spawn(move || {
                    let result = fetch_preview(client, parts, request_body);
                    let _ = sender.send(result);
                })?;
            receiver
                .await
                .map_err(|_| anyhow::anyhow!("image preview worker stopped"))?
        }
        .boxed()
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }
}

fn fetch_preview(
    client: reqwest::blocking::Client,
    parts: http::request::Parts,
    request_body: Vec<u8>,
) -> anyhow::Result<http::Response<AsyncBody>> {
    if parts.uri.scheme_str() != Some("https") {
        anyhow::bail!("image previews require HTTPS");
    }
    let mut request = client
        .request(parts.method, parts.uri.to_string())
        .headers(parts.headers);
    if !request_body.is_empty() {
        request = request.body(request_body);
    }
    let response = request.send()?;
    if response.url().scheme() != "https" {
        anyhow::bail!("image preview redirected to an insecure URL");
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PREVIEW_BYTES)
    {
        anyhow::bail!("image preview exceeds 16 MiB");
    }
    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    let mut bytes = Vec::new();
    response
        .take(MAX_PREVIEW_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PREVIEW_BYTES {
        anyhow::bail!("image preview exceeds 16 MiB");
    }
    let mut result = http::Response::builder().status(status).version(version);
    let result_headers = result
        .headers_mut()
        .ok_or_else(|| anyhow::anyhow!("cannot construct image preview response"))?;
    *result_headers = headers;
    Ok(result.body(AsyncBody::from(bytes))?)
}

struct BootableAssets;

impl AssetSource for BootableAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let asset = match path {
            "icons/window-minimize.svg" => Some(WINDOW_MINIMIZE_SVG),
            "icons/window-maximize.svg" => Some(WINDOW_MAXIMIZE_SVG),
            "icons/window-restore.svg" => Some(WINDOW_RESTORE_SVG),
            "icons/window-close.svg" => Some(WINDOW_CLOSE_SVG),
            "brand/bootable-mark.svg" => Some(BRAND_MARK_SVG),
            "brand/bootable-logo.svg" => Some(BRAND_LOGO_SVG),
            "ui/image.svg" => Some(IMAGE_ICON_SVG),
            "ui/usb.svg" => Some(USB_ICON_SVG),
            "ui/settings.svg" => Some(SETTINGS_ICON_SVG),
            "ui/refresh.svg" => Some(REFRESH_ICON_SVG),
            "ui/hash.svg" => Some(HASH_ICON_SVG),
            "ui/folder.svg" => Some(FOLDER_ICON_SVG),
            "ui/backup.svg" => Some(BACKUP_ICON_SVG),
            "ui/review.svg" => Some(REVIEW_ICON_SVG),
            "ui/discover.svg" => Some(DISCOVER_ICON_SVG),
            "ui/download.svg" => Some(DOWNLOAD_ICON_SVG),
            _ => None,
        };
        Ok(asset.map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        if path == "brand" {
            return Ok(vec![
                SharedString::from("bootable-mark.svg"),
                SharedString::from("bootable-logo.svg"),
            ]);
        }
        if path == "ui" {
            return Ok([
                "image.svg",
                "usb.svg",
                "settings.svg",
                "refresh.svg",
                "hash.svg",
                "folder.svg",
                "backup.svg",
                "review.svg",
                "discover.svg",
                "download.svg",
            ]
            .into_iter()
            .map(SharedString::from)
            .collect());
        }
        if path == "icons" {
            return Ok([
                "window-minimize.svg",
                "window-maximize.svg",
                "window-restore.svg",
                "window-close.svg",
            ]
            .into_iter()
            .map(SharedString::from)
            .collect());
        }
        Ok(Vec::new())
    }
}

struct BootableView {
    engine: Bootable,
    image: Option<ImageReport>,
    image_loading: bool,
    devices: Vec<Device>,
    selected_device: Option<usize>,
    browse_directory: Option<std::path::PathBuf>,
    options: WriteOptions,
    advanced: bool,
    checksum_algorithm: ChecksumAlgorithm,
    catalog_open: bool,
    distro_loading: bool,
    pi_loading: bool,
    directory_loading: bool,
    catalog_state: CatalogState,
    directory_state: CatalogState,
    pi_state: CatalogState,
    arch_state: CatalogState,
    debian_state: CatalogState,
    details_state: CatalogState,
    distributions: Vec<DistributionSummary>,
    popular_distributions: Vec<DistributionSummary>,
    distribution_directory: Vec<DistributionSummary>,
    selected_distribution: Option<usize>,
    selected_details: Option<DistributionDetails>,
    catalog_releases: Vec<IsoRelease>,
    selected_release: Option<usize>,
    discovery_source: DiscoverySource,
    quick_access: QuickAccess,
    arch_distributions: Vec<DistributionSummary>,
    debian_distributions: Vec<DistributionSummary>,
    arch_loading: bool,
    debian_loading: bool,
    pi_catalog: Option<PiCatalog>,
    selected_pi_device: Option<usize>,
    selected_pi_image: Option<usize>,
    catalog_search: Entity<InputState>,
    windows_partition_scheme: Entity<SelectState<Vec<&'static str>>>,
    catalog_visible: usize,
    pi_visible: usize,
    _catalog_search_subscription: Subscription,
    _windows_partition_subscription: Subscription,
    active_download: Option<Progress>,
    download_control: Option<OperationControl>,
    active_download_job: Option<String>,
    download_jobs: Vec<DownloadJob>,
    downloads_open: bool,
    review_plan: Option<WritePlan>,
    active_write: bool,
    write_control: Option<OperationControl>,
    write_progress: Option<Progress>,
    write_started_at: Option<Instant>,
    write_result: Option<Result<(), String>>,
    confirm_modal: bool,
    confirm_acknowledged: bool,
    status: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiscoverySource {
    DistroWatch,
    RaspberryPi,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QuickAccess {
    All,
    Arch,
    Debian,
    Omarchy,
    Windows,
}

#[derive(Clone, Copy)]
struct ViewportLayout {
    compact: bool,
    wide: bool,
    distribution_height: Pixels,
    screenshot_height: Pixels,
    release_height: Pixels,
}

impl ViewportLayout {
    fn new(width: Pixels, height: Pixels) -> Self {
        let compact = width < px(960.);
        let wide = width >= px(1_500.);
        let (screenshot_height, release_height) = if compact {
            (px(96.), px(132.))
        } else if height >= px(1_300.) {
            (px(112.), px(144.))
        } else if height >= px(1_000.) {
            (px(104.), px(136.))
        } else {
            (px(92.), px(120.))
        };
        Self {
            compact,
            wide,
            distribution_height: if height >= px(840.) {
                px(288.)
            } else {
                px(228.)
            },
            screenshot_height,
            release_height,
        }
    }
}

enum DownloadUpdate {
    Progress(Progress),
    Finished(Result<(ImageReport, std::path::PathBuf), String>),
}

enum WriteUpdate {
    Progress(Progress),
    Finished(Result<(), String>),
}

impl BootableView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = Bootable::native();
        let catalog_search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search distributions and images…")
                .clean_on_escape()
        });
        let search_subscription = cx.subscribe(&catalog_search, |view, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                view.catalog_visible = 20;
                view.pi_visible = 20;
                let query = input.read(cx).value();
                view.apply_distribution_search(query.as_ref(), cx);
            }
        });
        let windows_partition_scheme = cx.new(|cx| {
            SelectState::new(
                vec!["GPT · UEFI", "MBR · UEFI"],
                Some(gpui_component::IndexPath::default().row(0)),
                window,
                cx,
            )
        });
        let windows_partition_subscription =
            cx.subscribe(&windows_partition_scheme, |view, _, event, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    view.options.windows_partition_scheme = if value.starts_with("MBR") {
                        WindowsPartitionScheme::Mbr
                    } else {
                        WindowsPartitionScheme::Gpt
                    };
                    view.status = format!(
                        "Windows partition scheme: {} · target firmware: UEFI",
                        view.options.windows_partition_scheme
                    );
                    cx.notify();
                }
            });
        let (devices, status) = match engine.discover_devices() {
            Ok(devices) => {
                let eligible = devices
                    .iter()
                    .filter(|device| device.is_eligible_target())
                    .count();
                (
                    devices,
                    format!(
                        "Watching for USB changes • {eligible} eligible target(s) • nothing is written while planning"
                    ),
                )
            }
            Err(error) => (Vec::new(), error.to_string()),
        };
        Self::schedule_device_scan(cx);
        Self::schedule_download_scan(cx);
        let view = Self {
            engine,
            image: None,
            image_loading: false,
            devices,
            selected_device: None,
            browse_directory: None,
            options: WriteOptions::default(),
            advanced: false,
            checksum_algorithm: ChecksumAlgorithm::Sha256,
            catalog_open: true,
            distro_loading: false,
            pi_loading: false,
            directory_loading: false,
            catalog_state: CatalogState::Idle,
            directory_state: CatalogState::Idle,
            pi_state: CatalogState::Idle,
            arch_state: CatalogState::Idle,
            debian_state: CatalogState::Idle,
            details_state: CatalogState::Idle,
            distributions: Vec::new(),
            popular_distributions: Vec::new(),
            distribution_directory: Vec::new(),
            selected_distribution: None,
            selected_details: None,
            catalog_releases: Vec::new(),
            selected_release: None,
            discovery_source: DiscoverySource::DistroWatch,
            quick_access: QuickAccess::All,
            arch_distributions: Vec::new(),
            debian_distributions: Vec::new(),
            arch_loading: false,
            debian_loading: false,
            pi_catalog: None,
            selected_pi_device: None,
            selected_pi_image: None,
            catalog_search,
            windows_partition_scheme,
            catalog_visible: 20,
            pi_visible: 20,
            _catalog_search_subscription: search_subscription,
            _windows_partition_subscription: windows_partition_subscription,
            active_download: None,
            download_control: None,
            active_download_job: None,
            download_jobs: Vec::new(),
            downloads_open: false,
            review_plan: None,
            active_write: false,
            write_control: None,
            write_progress: None,
            write_started_at: None,
            write_result: None,
            confirm_modal: false,
            confirm_acknowledged: false,
            status,
        };
        Self::schedule_initial_catalog_load(cx);
        view
    }

    fn schedule_initial_catalog_load(cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx| {
            Timer::after(Duration::from_millis(1)).await;
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| {
                    view.refresh_download_jobs(cx);
                    view.start_next_queued_download(cx);
                    view.load_catalog(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn toggle_catalog(&mut self, cx: &mut Context<Self>) {
        self.catalog_open = !self.catalog_open;
        if !self.catalog_open {
            self.status = format!("Catalog closed • {}", self.review_readiness().guidance());
            cx.notify();
            return;
        }
        self.advanced = false;
        if self.distributions.is_empty() {
            self.load_catalog(cx);
        } else {
            self.status =
                "DistroWatch catalog ready • rankings indicate interest, not quality".into();
            cx.notify();
        }
    }

    fn load_catalog(&mut self, cx: &mut Context<Self>) {
        self.load_catalog_with(CacheMode::PreferCache, cx);
    }

    fn load_catalog_with(&mut self, mode: CacheMode, cx: &mut Context<Self>) {
        if self.distro_loading {
            return;
        }
        self.distro_loading = true;
        self.catalog_state = CatalogState::Loading;
        self.status = "Loading DistroWatch six-month popularity…".into();
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { Bootable::native().popular_distributions_cached(100, mode) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| {
                    view.distro_loading = false;
                    match result {
                        Ok(fetch) => {
                            view.catalog_state =
                                CatalogState::from_fetch(&fetch, fetch.value.is_empty());
                            let source = fetch.status_suffix();
                            let distributions = fetch.value;
                            let count = distributions.len();
                            view.popular_distributions = distributions.clone();
                            let showing_popular = view.catalog_search_query(cx).is_empty()
                                && view.quick_access == QuickAccess::All
                                && view.discovery_source == DiscoverySource::DistroWatch;
                            if showing_popular {
                                view.distributions = distributions;
                            }
                            view.status = format!("{count} distributions · {source}");
                            if count > 0 && showing_popular {
                                view.select_distribution(0, cx);
                            }
                        }
                        Err(error) => {
                            view.catalog_state = CatalogState::Failed(error.to_string());
                            view.status = view.catalog_state.short_label("distributions");
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn apply_distribution_search(&mut self, query: &str, cx: &mut Context<Self>) {
        self.selected_distribution = None;
        self.selected_details = None;
        self.selected_release = None;
        self.catalog_releases.clear();
        self.details_state = CatalogState::Idle;
        self.catalog_visible = 20;
        if query.trim().is_empty() {
            self.distributions = self.popular_distributions.clone();
            self.status = "Showing DistroWatch six-month popularity".into();
            cx.notify();
            return;
        }
        self.quick_access = QuickAccess::All;
        self.discovery_source = DiscoverySource::DistroWatch;
        if self.distribution_directory.is_empty() {
            self.load_distribution_directory(cx);
        } else {
            self.distributions = self.distribution_directory.clone();
            self.status = format!(
                "Searching {} distributions from DistroWatch",
                self.distribution_directory.len()
            );
            cx.notify();
        }
    }

    fn load_distribution_directory(&mut self, cx: &mut Context<Self>) {
        if self.directory_loading {
            return;
        }
        self.directory_loading = true;
        self.directory_state = CatalogState::Loading;
        self.status = "Searching DistroWatch's full distribution directory…".into();
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            Bootable::native().distribution_directory_cached(CacheMode::PreferCache)
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| {
                    view.directory_loading = false;
                    match result {
                        Ok(fetch) => {
                            view.directory_state =
                                CatalogState::from_fetch(&fetch, fetch.value.is_empty());
                            let directory = fetch.value;
                            let count = directory.len();
                            view.distribution_directory = directory.clone();
                            if !view.catalog_search_query(cx).is_empty()
                                && view.discovery_source == DiscoverySource::DistroWatch
                            {
                                view.distributions = directory;
                                view.status = format!("Searching {count} distributions");
                            }
                        }
                        Err(error) => {
                            view.directory_state = CatalogState::Failed(error.to_string());
                            if view.discovery_source == DiscoverySource::DistroWatch
                                && !view.catalog_search_query(cx).is_empty()
                            {
                                view.status = view.directory_state.short_label("search catalog");
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn show_raspberry_pi(&mut self, cx: &mut Context<Self>) {
        self.show_raspberry_pi_with(CacheMode::PreferCache, cx);
    }

    fn show_raspberry_pi_with(&mut self, mode: CacheMode, cx: &mut Context<Self>) {
        self.discovery_source = DiscoverySource::RaspberryPi;
        self.quick_access = QuickAccess::All;
        if (self.pi_catalog.is_some() && mode == CacheMode::PreferCache) || self.pi_loading {
            self.status = "Raspberry Pi image discovery selected".into();
            cx.notify();
            return;
        }
        self.pi_loading = true;
        self.pi_state = CatalogState::Loading;
        self.status = "Loading the official Raspberry Pi Imager catalog…".into();
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { Bootable::native().raspberry_pi_catalog_cached(mode) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| {
                    view.pi_loading = false;
                    match result {
                        Ok(fetch) => {
                            view.pi_state =
                                CatalogState::from_fetch(&fetch, fetch.value.images.is_empty());
                            let source = fetch.status_suffix();
                            let catalog = fetch.value;
                            let count = catalog.images.len();
                            view.pi_catalog = Some(catalog);
                            view.selected_pi_device = None;
                            view.selected_pi_image = (count > 0).then_some(0);
                            if view.discovery_source == DiscoverySource::RaspberryPi {
                                view.status = format!("{count} Raspberry Pi images · {source}");
                            }
                        }
                        Err(error) => {
                            view.pi_state = CatalogState::Failed(error.to_string());
                            if view.discovery_source == DiscoverySource::RaspberryPi {
                                view.status = view.pi_state.short_label("Raspberry Pi images");
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn show_quick_access(
        &mut self,
        preset: QuickAccess,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.discovery_source = DiscoverySource::DistroWatch;
        self.quick_access = preset;
        self.catalog_search.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.selected_distribution = None;
        self.selected_details = None;
        self.catalog_releases.clear();
        self.details_state = CatalogState::Idle;
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
                    self.load_quick_base(preset, cx);
                    return;
                }
                self.distributions = cached.clone();
                self.status = format!(
                    "Showing {} active {}-based distributions from DistroWatch",
                    cached.len(),
                    if preset == QuickAccess::Arch {
                        "Arch"
                    } else {
                        "Debian"
                    }
                );
            }
            QuickAccess::Omarchy => {
                let omarchy = self
                    .distribution_directory
                    .iter()
                    .chain(self.popular_distributions.iter())
                    .find(|distribution| distribution.slug == "omarchy")
                    .cloned()
                    .unwrap_or_else(|| DistributionSummary {
                        rank: 0,
                        name: "Omarchy".into(),
                        slug: "omarchy".into(),
                        hits_per_day: 0,
                        based_on: Some("Arch".into()),
                        page_url: "https://distrowatch.com/table.php?distribution=omarchy".into(),
                        logo_url: "https://distrowatch.com/images/icon-large/omarchy.png".into(),
                    });
                self.distributions = vec![omarchy];
                self.status = "Omarchy family · ISO releases are writable; installer-only derivatives are clearly marked".into();
            }
            QuickAccess::Windows => {
                self.distributions.clear();
                self.status =
                    "Windows media tools · choose a Windows ISO to unlock setup options".into();
            }
        }
        cx.notify();
    }

    fn load_quick_base(&mut self, preset: QuickAccess, cx: &mut Context<Self>) {
        self.load_quick_base_with(preset, CacheMode::PreferCache, cx);
    }

    fn load_quick_base_with(
        &mut self,
        preset: QuickAccess,
        mode: CacheMode,
        cx: &mut Context<Self>,
    ) {
        let loading = if preset == QuickAccess::Arch {
            self.arch_loading
        } else {
            self.debian_loading
        };
        if loading {
            return;
        }
        let base = if preset == QuickAccess::Arch {
            "Arch"
        } else {
            "Debian"
        };
        if preset == QuickAccess::Arch {
            self.arch_loading = true;
            self.arch_state = CatalogState::Loading;
        } else {
            self.debian_loading = true;
            self.debian_state = CatalogState::Loading;
        }
        self.status = format!("Searching DistroWatch for active {base}-based distributions…");
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { Bootable::native().distributions_based_on_cached(base, mode) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| {
                    if preset == QuickAccess::Arch {
                        view.arch_loading = false;
                    } else {
                        view.debian_loading = false;
                    }
                    match result {
                        Ok(fetch) => {
                            let state = CatalogState::from_fetch(&fetch, fetch.value.is_empty());
                            let source = fetch.status_suffix();
                            let distributions = fetch.value;
                            let count = distributions.len();
                            if preset == QuickAccess::Arch {
                                view.arch_distributions = distributions.clone();
                                view.arch_state = state;
                            } else {
                                view.debian_distributions = distributions.clone();
                                view.debian_state = state;
                            }
                            if view.quick_access == preset {
                                view.distributions = distributions;
                                view.status =
                                    format!("{count} active {base}-based distributions · {source}");
                            }
                        }
                        Err(error) => {
                            let state = CatalogState::Failed(error.to_string());
                            if preset == QuickAccess::Arch {
                                view.arch_state = state.clone();
                            } else {
                                view.debian_state = state.clone();
                            }
                            if view.quick_access == preset {
                                view.status =
                                    state.short_label(&format!("{base}-based distributions"));
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn select_pi_device(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        self.selected_pi_device = index;
        self.selected_pi_image = self.visible_pi_images().first().map(|(index, _)| *index);
        self.status = match index.and_then(|index| self.pi_catalog.as_ref()?.devices.get(index)) {
            Some(device) => format!("Showing images compatible with {}", device.name),
            None => "Showing every Raspberry Pi image".into(),
        };
        cx.notify();
    }

    fn visible_pi_images(&self) -> Vec<(usize, &PiImage)> {
        let selected_tags = self
            .selected_pi_device
            .and_then(|index| self.pi_catalog.as_ref()?.devices.get(index))
            .map(|device| device.tags.as_slice());
        self.pi_catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .images
                    .iter()
                    .enumerate()
                    .filter(|(_, image)| {
                        selected_tags.is_none_or(|tags| {
                            image.devices.is_empty()
                                || image
                                    .devices
                                    .iter()
                                    .any(|tag| tags.iter().any(|selected| selected == tag))
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn catalog_search_query(&self, cx: &App) -> String {
        self.catalog_search.read(cx).value().trim().to_lowercase()
    }

    fn load_more_discovery(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let scrolling_down = match event.delta {
            ScrollDelta::Pixels(delta) => delta.y < px(0.),
            ScrollDelta::Lines(delta) => delta.y < 0.,
        };
        if !scrolling_down {
            return;
        }
        match self.discovery_source {
            DiscoverySource::DistroWatch => {
                self.catalog_visible = self.catalog_visible.saturating_add(20);
            }
            DiscoverySource::RaspberryPi => {
                self.pi_visible = self.pi_visible.saturating_add(20);
            }
        }
        cx.notify();
    }

    fn observe_download(
        &mut self,
        mut receiver: mpsc::UnboundedReceiver<DownloadUpdate>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |view, cx| {
            while let Some(update) = receiver.next().await {
                let Some(view) = view.upgrade() else {
                    break;
                };
                view.update(cx, |view, cx| {
                    match update {
                        DownloadUpdate::Progress(progress) => {
                            view.status = progress.message.clone();
                            view.active_download = Some(progress);
                        }
                        DownloadUpdate::Finished(result) => {
                            view.active_download = None;
                            view.download_control = None;
                            view.active_download_job = None;
                            match result {
                                Ok((report, destination)) => {
                                    view.browse_directory = destination
                                        .parent()
                                        .map(std::path::PathBuf::from);
                                    view.status = format!(
                                        "Ready · downloaded, verified, and inspected {} · discovery remains open",
                                        report.path.display()
                                    );
                                    view.image = Some(report);
                                    view.advanced = false;
                                }
                                Err(error) if error.contains("cancelled safely") => {
                                    view.status =
                                        "Download cancelled • temporary data cleaned up".into();
                                }
                                Err(error) => {
                                    view.status = format!("Download stopped · {error}");
                                }
                            }
                            view.refresh_download_jobs(cx);
                            view.start_next_queued_download(cx);
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn refresh_download_jobs(&mut self, cx: &mut Context<Self>) {
        match self.engine.download_jobs() {
            Ok(jobs) => self.download_jobs = jobs,
            Err(error) => self.status = format!("Download history unavailable · {error}"),
        }
        cx.notify();
    }

    fn toggle_downloads(&mut self, cx: &mut Context<Self>) {
        self.downloads_open = !self.downloads_open;
        if self.downloads_open {
            self.refresh_download_jobs(cx);
            self.status = format!("{} download job(s) in history", self.download_jobs.len());
        }
        cx.notify();
    }

    fn launch_download_job(
        &mut self,
        id: String,
        destination: std::path::PathBuf,
        retry: bool,
        cx: &mut Context<Self>,
    ) {
        if self.download_control.is_some() {
            self.status = "Download queued · it will start when the active job finishes".into();
            self.refresh_download_jobs(cx);
            cx.notify();
            return;
        }
        let control = OperationControl::new();
        self.download_control = Some(control.clone());
        self.active_download_job = Some(id.clone());
        self.active_download = None;
        self.status = if retry {
            "Retrying download · preserved bytes will be resumed when supported".into()
        } else {
            "Starting managed download…".into()
        };
        let completed_destination = destination.clone();
        let (sender, receiver) = mpsc::unbounded();
        cx.background_executor()
            .spawn(async move {
                let progress_sender = sender.clone();
                let engine = Bootable::native();
                let result = if retry {
                    engine.retry_download_job(&id, &control, move |progress| {
                        let _ = progress_sender.unbounded_send(DownloadUpdate::Progress(progress));
                    })
                } else {
                    engine.run_download_job(&id, &control, move |progress| {
                        let _ = progress_sender.unbounded_send(DownloadUpdate::Progress(progress));
                    })
                }
                .map(|report| (report, completed_destination))
                .map_err(|error| error.to_string());
                let _ = sender.unbounded_send(DownloadUpdate::Finished(result));
            })
            .detach();
        self.observe_download(receiver, cx);
        cx.notify();
    }

    fn retry_managed_download(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(job) = self.download_jobs.iter().find(|job| job.id == id) else {
            self.status = "Download job no longer exists".into();
            cx.notify();
            return;
        };
        if !job.status.can_retry() {
            self.status = format!("{} cannot be retried while it is {}", job.label, job.status);
            cx.notify();
            return;
        }
        let destination = job.destination.clone();
        if self.download_control.is_some() {
            match self.engine.queue_download_retry(&id) {
                Ok(()) => {
                    self.status = "Retry queued · it will start after the active download".into();
                    self.refresh_download_jobs(cx);
                }
                Err(error) => {
                    self.status = error.to_string();
                    cx.notify();
                }
            }
        } else {
            self.launch_download_job(id, destination, true, cx);
        }
    }

    fn start_next_queued_download(&mut self, cx: &mut Context<Self>) {
        if self.download_control.is_some() {
            return;
        }
        match self.engine.next_queued_download() {
            Ok(Some(job)) => self.launch_download_job(job.id, job.destination, false, cx),
            Ok(None) => {}
            Err(error) => {
                self.status = format!("Could not start queued download · {error}");
                cx.notify();
            }
        }
    }

    fn use_managed_download(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(job) = self.download_jobs.iter().find(|job| job.id == id) else {
            self.status = "Download job no longer exists".into();
            cx.notify();
            return;
        };
        if job.status != DownloadStatus::Completed {
            self.status = "Only completed downloads can be selected".into();
            cx.notify();
            return;
        }
        match self.engine.inspect_image(&job.destination) {
            Ok(report) => {
                self.browse_directory = job.destination.parent().map(std::path::PathBuf::from);
                self.image = Some(report);
                self.advanced = false;
                self.downloads_open = false;
                self.status = format!("Using completed download {}", job.destination.display());
            }
            Err(error) => self.status = format!("Downloaded image is unavailable · {error}"),
        }
        cx.notify();
    }

    fn remove_managed_download(&mut self, id: &str, cx: &mut Context<Self>) {
        match self.engine.remove_download_job(id) {
            Ok(()) => {
                self.status = "Download history entry removed · completed image kept".into();
                self.refresh_download_jobs(cx);
            }
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
            }
        }
    }

    fn download_pi_catalog_image(&mut self, cx: &mut Context<Self>) {
        let Some(image) = self
            .selected_pi_image
            .and_then(|index| self.pi_catalog.as_ref()?.images.get(index))
            .cloned()
        else {
            self.status = "Choose a Raspberry Pi image to download".into();
            cx.notify();
            return;
        };
        let mut dialog = rfd::FileDialog::new().set_file_name(&image.suggested_filename);
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        let Some(destination) = dialog.save_file() else {
            self.status = "Raspberry Pi image download cancelled".into();
            cx.notify();
            return;
        };
        match self.engine.enqueue_pi_download(&image, &destination) {
            Ok(id) => self.launch_download_job(id, destination, false, cx),
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
            }
        }
    }

    fn select_distribution(&mut self, index: usize, cx: &mut Context<Self>) {
        self.select_distribution_with(index, CacheMode::PreferCache, cx);
    }

    fn select_distribution_with(&mut self, index: usize, mode: CacheMode, cx: &mut Context<Self>) {
        let Some(distribution) = self.distributions.get(index).cloned() else {
            return;
        };
        self.selected_distribution = Some(index);
        self.selected_details = None;
        self.selected_release = None;
        self.catalog_releases.clear();
        self.distro_loading = true;
        self.details_state = CatalogState::Loading;
        self.status = format!("Resolving current {} ISO files…", distribution.name);
        cx.notify();
        let request_slug = distribution.slug.clone();
        let fetch_slug = request_slug.clone();
        let task = cx
            .background_executor()
            .spawn(async move { Bootable::native().distribution_bundle_cached(&fetch_slug, mode) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| {
                    let selected_slug = view
                        .selected_distribution
                        .and_then(|index| view.distributions.get(index))
                        .map(|distribution| distribution.slug.as_str());
                    if selected_slug != Some(request_slug.as_str()) {
                        return;
                    }
                    view.distro_loading = false;
                    match result {
                        Ok(fetch) => {
                            view.details_state =
                                CatalogState::from_fetch(&fetch, fetch.value.releases.is_empty());
                            let source = fetch.status_suffix();
                            let DistributionBundle {
                                details,
                                releases,
                                warnings,
                            } = fetch.value;
                            let count = releases.len();
                            view.selected_details = Some(details);
                            view.catalog_releases = releases;
                            view.selected_release = (count > 0).then_some(0);
                            view.status = if count == 0 && !warnings.is_empty() {
                                format!(
                                    "Profile ready · no direct ISO found · {} source error(s)",
                                    warnings.len()
                                )
                            } else if count == 0 {
                                "Profile loaded • no direct ISO was resolved from its current links"
                                    .into()
                            } else if !warnings.is_empty() {
                                format!(
                                    "{count} direct ISO release(s) · {source} · {} source warning(s)",
                                    warnings.len()
                                )
                            } else {
                                format!("{count} direct ISO release(s) · {source}")
                            };
                        }
                        Err(error) => {
                            view.details_state = CatalogState::Failed(error.to_string());
                            view.status = view.details_state.short_label("ISO releases");
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn open_selected_distrowatch_page(&mut self, cx: &mut Context<Self>) {
        let Some(page_url) = self
            .selected_distribution
            .and_then(|index| self.distributions.get(index))
            .map(|distribution| distribution.page_url.clone())
        else {
            self.status = "Choose a distribution first".into();
            cx.notify();
            return;
        };
        self.status = match self.engine.open_distrowatch_page(&page_url) {
            Ok(()) => "Opened the DistroWatch distribution page in your browser".into(),
            Err(error) => error.to_string(),
        };
        cx.notify();
    }

    fn retry_discovery(&mut self, cx: &mut Context<Self>) {
        if self.discovery_source == DiscoverySource::RaspberryPi {
            self.show_raspberry_pi_with(CacheMode::Refresh, cx);
            return;
        }
        match self.quick_access {
            QuickAccess::All | QuickAccess::Omarchy => {
                if let Some(index) = self.selected_distribution {
                    self.select_distribution_with(index, CacheMode::Refresh, cx);
                } else {
                    self.load_catalog_with(CacheMode::Refresh, cx);
                }
            }
            QuickAccess::Arch | QuickAccess::Debian => {
                self.load_quick_base_with(self.quick_access, CacheMode::Refresh, cx);
            }
            QuickAccess::Windows => {
                self.status = "Windows tools use the selected local ISO".into();
                cx.notify();
            }
        }
    }

    fn download_catalog_release(&mut self, cx: &mut Context<Self>) {
        let Some(release) = self
            .selected_release
            .and_then(|index| self.catalog_releases.get(index))
            .cloned()
        else {
            self.status = "Choose an ISO release to download".into();
            cx.notify();
            return;
        };
        let mut dialog = rfd::FileDialog::new().set_file_name(&release.name);
        if let Some(directory) = &self.browse_directory {
            dialog = dialog.set_directory(directory);
        }
        let Some(destination) = dialog.save_file() else {
            self.status = "ISO download cancelled".into();
            cx.notify();
            return;
        };
        match self.engine.enqueue_iso_download(&release, &destination) {
            Ok(id) => self.launch_download_job(id, destination, false, cx),
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
            }
        }
    }

    fn toggle_download_pause(&mut self, cx: &mut Context<Self>) {
        let Some(control) = &self.download_control else {
            return;
        };
        match control.state() {
            OperationState::Running => {
                control.pause();
                if let Some(id) = self.active_download_job.as_deref() {
                    let _ = self.engine.set_download_paused(id, true);
                }
                self.status = "Download paused • resume or cancel when ready".into();
            }
            OperationState::Paused => {
                control.resume();
                if let Some(id) = self.active_download_job.as_deref() {
                    let _ = self.engine.set_download_paused(id, false);
                }
                self.status = "Download resumed".into();
            }
            OperationState::Cancelled => {}
        }
        cx.notify();
    }

    fn cancel_download(&mut self, cx: &mut Context<Self>) {
        let Some(control) = &self.download_control else {
            return;
        };
        control.cancel();
        self.status = "Cancelling download safely • cleaning temporary data…".into();
        cx.notify();
    }

    fn schedule_device_scan(cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx| {
            Timer::after(DEVICE_SCAN_INTERVAL).await;
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| {
                    view.scan_devices(false, cx);
                    Self::schedule_device_scan(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn schedule_download_scan(cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx| {
            Timer::after(DOWNLOAD_SCAN_INTERVAL).await;
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| {
                    if view.downloads_open
                        || view.download_control.is_some()
                        || view.download_jobs.iter().any(|job| {
                            matches!(
                                job.status,
                                DownloadStatus::Queued
                                    | DownloadStatus::Running
                                    | DownloadStatus::Paused
                            )
                        })
                    {
                        view.refresh_download_jobs(cx);
                        view.start_next_queued_download(cx);
                    }
                    Self::schedule_download_scan(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn choose_image(&mut self, cx: &mut Context<Self>) {
        if self.image_loading {
            self.status = "Image inspection is already running".into();
            cx.notify();
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
        let selected = dialog.pick_file();
        if let Some(path) = selected {
            self.image_loading = true;
            self.status =
                "Inspecting image • compressed sources are measured after expansion…".into();
            cx.notify();
            let inspected_path = path.clone();
            let task = cx.background_executor().spawn(async move {
                Bootable::native()
                    .inspect_image(&inspected_path)
                    .map_err(|error| error.to_string())
            });
            cx.spawn(async move |view, cx| {
                let result = task.await;
                if let Some(view) = view.upgrade() {
                    view.update(cx, |view, cx| {
                        view.image_loading = false;
                        match result {
                            Ok(report) => {
                                view.status = format!("Recognized {}", report.kind);
                                view.browse_directory = path.parent().map(std::path::PathBuf::from);
                                view.image = Some(report);
                                view.advanced = false;
                            }
                            Err(error) => {
                                view.image = None;
                                view.advanced = false;
                                view.status = error;
                            }
                        }
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
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
        cx.notify();
    }

    fn backup_device(&mut self, cx: &mut Context<Self>) {
        let Some(device) = self
            .selected_device
            .and_then(|index| self.devices.get(index))
            .cloned()
        else {
            self.status = "Choose a removable drive to back up".into();
            cx.notify();
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
            cx.notify();
            return;
        };
        self.browse_directory = destination.parent().map(std::path::PathBuf::from);
        self.status = format!("Backing up {} in the background…", device.display_name());
        cx.notify();

        let device_id = device.id.to_string();
        let completed_destination = destination.clone();
        let task = cx.background_executor().spawn(async move {
            Bootable::native()
                .backup_device(&device_id, &destination, |_| {})
                .map(|()| completed_destination)
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| {
                    view.status = match result {
                        Ok(destination) => {
                            format!("Drive image saved to {}", destination.display())
                        }
                        Err(error) => error.to_string(),
                    };
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn checksum_image(&mut self, cx: &mut Context<Self>) {
        let Some(image) = &self.image else {
            self.status = "Choose an image before computing its checksum".into();
            cx.notify();
            return;
        };
        self.status = match self
            .engine
            .checksum_image(&image.path, self.checksum_algorithm)
        {
            Ok(checksum) => format!("{}: {}", checksum.algorithm, checksum.hexadecimal),
            Err(error) => error.to_string(),
        };
        cx.notify();
    }

    fn cycle_checksum_algorithm(&mut self, cx: &mut Context<Self>) {
        self.checksum_algorithm = self.checksum_algorithm.next();
        self.status = format!("Checksum algorithm: {}", self.checksum_algorithm);
        cx.notify();
    }

    fn toggle_advanced(&mut self, cx: &mut Context<Self>) {
        if self.image.is_none() {
            self.advanced = false;
            self.status = "Choose or download an image before opening media options".into();
            cx.notify();
            return;
        }
        self.advanced = !self.advanced;
        self.status = if self.advanced {
            "Advanced options expanded • every choice is included in the reviewed plan".into()
        } else {
            "Advanced options collapsed • configured values remain active".into()
        };
        cx.notify();
    }

    fn cycle_bad_blocks(&mut self, cx: &mut Context<Self>) {
        self.options.bad_block_check = self.options.bad_block_check.next();
        self.status = match self.options.bad_block_check {
            BadBlockCheck::Disabled => "Destructive bad-block check disabled".into(),
            mode => format!(
                "Bad-block check: {} destructive pattern(s) before writing",
                mode.passes()
            ),
        };
        cx.notify();
    }

    fn refresh_devices(&mut self, cx: &mut Context<Self>) {
        self.scan_devices(true, cx);
    }

    fn scan_devices(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.active_write {
            if manual {
                self.status =
                    "Drive refresh is paused while writing • do not unplug the target".into();
                cx.notify();
            }
            return;
        }
        match self.engine.discover_devices() {
            Ok(devices) => {
                if devices == self.devices {
                    if manual {
                        self.status = "Drive list is up to date • automatic detection is on".into();
                        cx.notify();
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
                    .selected_device
                    .and_then(|index| self.devices.get(index))
                    .map(|device| device.id.clone());
                self.selected_device =
                    selected_id.and_then(|id| devices.iter().position(|device| device.id == id));
                self.devices = devices;
                self.status = device_change_message(added, removed);
            }
            Err(error) => self.status = error.to_string(),
        }
        cx.notify();
    }

    fn preview_plan(&mut self, cx: &mut Context<Self>) {
        let Some(image) = self.image.clone() else {
            self.status = "Choose an image first".into();
            cx.notify();
            return;
        };
        let Some(device) = self
            .selected_device
            .and_then(|index| self.devices.get(index))
            .cloned()
        else {
            self.status = "Choose a target drive first".into();
            cx.notify();
            return;
        };
        self.status = match self
            .engine
            .plan_with_options(image, device, self.options.clone())
        {
            Ok(plan) => {
                self.catalog_open = false;
                self.review_plan = Some(plan);
                self.active_write = false;
                self.write_progress = None;
                self.write_started_at = None;
                self.write_result = None;
                self.confirm_modal = false;
                self.confirm_acknowledged = false;
                "Reviewing the write plan • nothing has been written".into()
            }
            Err(error) => error.to_string(),
        };
        cx.notify();
    }

    fn review_readiness(&self) -> ReviewReadiness {
        review_readiness(
            self.image.as_ref(),
            self.selected_device
                .and_then(|index| self.devices.get(index)),
        )
    }

    fn close_review(&mut self, cx: &mut Context<Self>) {
        if self.active_write {
            self.status = "Writing is active • do not close the app or unplug the target".into();
            cx.notify();
            return;
        }
        self.review_plan = None;
        self.write_progress = None;
        self.write_started_at = None;
        self.write_result = None;
        self.confirm_modal = false;
        self.confirm_acknowledged = false;
        self.status = self.review_readiness().guidance().into();
        cx.notify();
    }

    fn open_write_confirmation(&mut self, cx: &mut Context<Self>) {
        if self.active_write || matches!(self.write_result, Some(Ok(()))) {
            return;
        }
        self.confirm_modal = true;
        self.confirm_acknowledged = false;
        self.status = "Review the target changes and consequences before writing".into();
        cx.notify();
    }

    fn close_write_confirmation(&mut self, cx: &mut Context<Self>) {
        self.confirm_modal = false;
        self.confirm_acknowledged = false;
        self.status = "Write cancelled before erasure • the target is unchanged".into();
        cx.notify();
    }

    fn start_write(&mut self, cx: &mut Context<Self>) {
        if self.active_write || matches!(self.write_result, Some(Ok(()))) {
            return;
        }
        let Some(plan) = self.review_plan.clone() else {
            self.status = "Review the write plan before writing".into();
            cx.notify();
            return;
        };
        if !self.confirm_modal
            || !destructive_confirmation_ready(
                self.confirm_acknowledged,
                self.active_write,
                matches!(self.write_result, Some(Ok(()))),
            )
        {
            self.status = "Acknowledge the consequences before confirming the write".into();
            cx.notify();
            return;
        }
        let confirmation = plan.confirmation_phrase.clone();

        let control = OperationControl::new();
        self.active_write = true;
        self.write_control = Some(control.clone());
        self.confirm_modal = false;
        self.confirm_acknowledged = false;
        self.write_progress = Some(Progress {
            phase: bootable_core::ProgressPhase::Preparing,
            completed: 0,
            total: Some(plan.image.size),
            message: "Waiting for administrator authentication, then revalidating the target"
                .into(),
        });
        self.write_started_at = Some(Instant::now());
        self.write_result = None;
        self.status = "Write started • do not unplug the target".into();
        cx.notify();

        let (sender, receiver) = mpsc::unbounded();
        cx.background_executor()
            .spawn(async move {
                let progress_sender = sender.clone();
                let result = Bootable::native()
                    .write_with_privilege_controlled(
                        &plan,
                        &confirmation,
                        &control,
                        move |progress| {
                            let _ = progress_sender.unbounded_send(WriteUpdate::Progress(progress));
                        },
                    )
                    .map_err(|error| error.to_string());
                let _ = sender.unbounded_send(WriteUpdate::Finished(result));
            })
            .detach();
        self.observe_write(receiver, cx);
    }

    fn observe_write(
        &mut self,
        mut receiver: mpsc::UnboundedReceiver<WriteUpdate>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |view, cx| {
            while let Some(update) = receiver.next().await {
                let Some(view) = view.upgrade() else {
                    break;
                };
                view.update(cx, |view, cx| {
                    match update {
                        WriteUpdate::Progress(progress) => {
                            view.status = format!("{} • {}", progress.phase, progress.message);
                            view.write_progress = Some(progress);
                        }
                        WriteUpdate::Finished(result) => {
                            view.active_write = false;
                            view.write_control = None;
                            if result.is_err() {
                                view.write_progress = None;
                                view.write_started_at = None;
                            }
                            view.write_result = Some(result.clone());
                            view.status = match result {
                                Ok(()) => {
                                    "Complete • image written and verified • target can be safely removed"
                                        .into()
                                }
                                Err(error) if error.contains("authentication") => {
                                    format!("Write cancelled before erasure • {error}")
                                }
                                Err(error) if error.contains("cancelled safely") => {
                                    "Write stopped safely • media is incomplete and must be rewritten before use"
                                        .into()
                                }
                                Err(error) => format!("Write failed • {error}"),
                            };
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn cancel_write(&mut self, cx: &mut Context<Self>) {
        let Some(control) = &self.write_control else {
            return;
        };
        control.cancel();
        self.status =
            "Stopping safely • flushing completed writes; the media will remain incomplete".into();
        cx.notify();
    }

    fn review_card(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let plan = self
            .review_plan
            .as_ref()
            .expect("review card is rendered only with a prepared plan");
        let write_succeeded = matches!(self.write_result, Some(Ok(())));
        let elapsed = self
            .write_started_at
            .map(|started| started.elapsed())
            .unwrap_or_default();
        let step_rows = plan
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .bg(rgb(0x0d151f))
                    .child(format!("{}. {}", index + 1, step.title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if step.destructive {
                                rgb(0xe5b95f)
                            } else {
                                rgb(0x8fa4bd)
                            })
                            .child(if step.destructive {
                                "ERASES DATA"
                            } else {
                                "safe"
                            }),
                    )
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .border_1()
            .border_color(rgb(0x243244))
            .bg(rgb(0x111923))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::BOLD)
                                    .child("Review write plan"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x8fa4bd))
                                    .child("Nothing is written until a separate destructive confirmation succeeds."),
                            ),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_3()
                    .child(review_value(
                        "SOURCE",
                        format!(
                            "{}\n{} • {}",
                            plan.image.path.display(),
                            plan.image.kind,
                            format_bytes(plan.image.size)
                        ),
                    ))
                    .child(review_value(
                        "TARGET",
                        format!(
                            "{}\n{} • {}",
                            plan.target.path.display(),
                            plan.target.display_name(),
                            format_bytes(plan.target.capacity)
                        ),
                    ))
                    .child(review_value("METHOD", plan.strategy.to_string()))
                    .child(review_value(
                        "CONSEQUENCE",
                        "All existing data and partitions on the selected target will be erased"
                            .into(),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Ordered operations"),
                    )
                    .children(step_rows),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(0x6b5428))
                    .bg(rgb(0x15140f))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xe5b95f))
                                    .child(if self.active_write {
                                        "Writing and verification are active"
                                    } else {
                                        "One final confirmation is required"
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xb6a17a))
                                    .child(if self.active_write {
                                        "Do not close the app, power off, or unplug the target drive."
                                    } else {
                                        "Review the exact target changes and irreversible consequences before writing."
                                    }),
                            ),
                    ),
            )
            .when_some(self.write_progress.as_ref(), |panel, progress| {
                let ratio = progress.ratio().unwrap_or_default() as f32;
                panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(0x243244))
                        .bg(rgb(0x0d151f))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .child(format!("{} · {}", progress.phase, progress.message)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0x8fa4bd))
                                        .child(progress.metrics(elapsed)),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .h(px(8.))
                                .rounded_full()
                                .bg(rgb(0x243244))
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(ratio))
                                        .rounded_full()
                                        .bg(rgb(if write_succeeded { 0x5bd7c0 } else { 0xe5b95f })),
                                ),
                        ),
                )
            })
            .when_some(self.write_result.as_ref(), |panel, result| {
                let (title, message, color) = match result {
                    Ok(()) => (
                        "Write complete",
                        "The image was written and verified. The removable drive can now be safely removed."
                            .to_string(),
                        0x5bd7c0,
                    ),
                    Err(error) => (
                        "Write stopped safely",
                        error.clone(),
                        0xf29a9a,
                    ),
                };
                panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(color))
                        .bg(rgb(0x0d151f))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(color))
                                .child(title),
                        )
                        .child(div().text_xs().text_color(rgb(0xa9b8c9)).child(message)),
                )
            })
    }

    fn review_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let write_succeeded = matches!(self.write_result, Some(Ok(())));
        let action_label = if self.active_write {
            "Stop safely"
        } else if write_succeeded {
            "Written & verified"
        } else if self.write_result.is_some() {
            "Review & retry"
        } else {
            "Review consequences"
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .px_4()
            .py_2()
            .rounded_xl()
            .border_1()
            .border_color(rgb(0x243244))
            .bg(rgb(0x0f1925))
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(rgb(if self.active_write {
                        0xe5b95f
                    } else {
                        0xa9b8c9
                    }))
                    .child(if self.active_write {
                        "Writing and verification are active · do not unplug the target"
                    } else if write_succeeded {
                        "Complete · the written media passed byte verification"
                    } else {
                        "Review the physical target and permanent changes before writing"
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("review-back")
                            .label("Back")
                            .disabled(self.active_write)
                            .on_click(cx.listener(|this, _, _, cx| this.close_review(cx))),
                    )
                    .child(
                        Button::new("review-consequences")
                            .danger()
                            .label(action_label)
                            .disabled(write_succeeded)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.active_write {
                                    this.cancel_write(cx);
                                } else {
                                    this.open_write_confirmation(cx);
                                }
                            })),
                    ),
            )
    }

    fn write_confirmation_modal(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let plan = self
            .review_plan
            .as_ref()
            .expect("confirmation modal requires a reviewed plan");
        let change_rows = plan
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(rgb(0x0d151f))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.))
                            .rounded_full()
                            .bg(rgb(if step.destructive { 0x4a291d } else { 0x183932 }))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(if step.destructive { 0xf29a9a } else { 0x5bd7c0 }))
                            .child((index + 1).to_string()),
                    )
                    .child(div().flex_1().text_sm().child(step.title.clone()))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if step.destructive { 0xf29a9a } else { 0x8fa4bd }))
                            .child(if step.destructive {
                                "ERASES DATA"
                            } else {
                                "verifies"
                            }),
                    )
            })
            .collect::<Vec<_>>();
        let consequences = [
            "Every existing file and partition on this physical drive will be permanently erased.",
            "Choosing the wrong drive destroys the data on that drive; confirm its model, path, and capacity below.",
            "Power loss, closing the app, or unplugging during writing can leave incomplete and unbootable media.",
            "Bootable rechecks the target identity immediately before erasure and verifies the result afterward.",
        ]
        .into_iter()
        .map(|message| {
            div()
                .flex()
                .items_start()
                .gap_3()
                .child(
                    div()
                        .mt_1()
                        .size(px(7.))
                        .rounded_full()
                        .bg(rgb(0xe5b95f)),
                )
                .child(div().flex_1().text_sm().text_color(rgb(0xc7b58f)).child(message))
        })
        .collect::<Vec<_>>();

        div()
            .id("write-confirmation-backdrop")
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .bg(rgba(0x000000cc))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .w_full()
                    .max_w(px(720.))
                    .max_h(relative(0.92))
                    .overflow_y_scrollbar()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_5()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(0x8a642b))
                    .bg(rgb(0x111923))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0xf0cc7d))
                                            .child("Confirm permanent changes"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(0x8fa4bd))
                                            .child("Review what Bootable will change and what can go wrong."),
                                    ),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_full()
                                    .bg(rgb(0x4a291d))
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xf29a9a))
                                    .child("PERMANENT"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_4()
                            .p_4()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(0x513c24))
                            .bg(rgb(0x18150f))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0xb6a17a))
                                            .child("PHYSICAL TARGET"),
                                    )
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(FontWeight::BOLD)
                                            .child(plan.target.display_name()),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(0x8fa4bd))
                                            .child(plan.target.path.display().to_string()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xf0cc7d))
                                    .child(format_bytes(plan.target.capacity)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .child("Changes to this drive"),
                            )
                            .children(change_rows),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_4()
                            .rounded_lg()
                            .bg(rgb(0x18150f))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xe5b95f))
                                    .child("Consequences"),
                            )
                            .children(consequences),
                    )
                    .child(
                        Checkbox::new("acknowledge-write-consequences")
                            .checked(self.confirm_acknowledged)
                            .label("I checked the physical target and understand that all of its existing data will be permanently erased.")
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.confirm_acknowledged = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_3()
                            .child(
                                Button::new("cancel-confirm-write")
                                    .label("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_write_confirmation(cx)
                                    })),
                            )
                            .child(
                                Button::new("confirm-write")
                                    .danger()
                                    .label("Confirm erase & write")
                                    .disabled(!destructive_confirmation_ready(
                                        self.confirm_acknowledged,
                                        self.active_write,
                                        matches!(self.write_result, Some(Ok(()))),
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| this.start_write(cx))),
                            ),
                    ),
            )
    }

    fn distrowatch_catalog_card(
        &self,
        cx: &mut Context<Self>,
        layout: ViewportLayout,
    ) -> impl IntoElement {
        let show_page_fallback = self.selected_distribution.is_some()
            && self.catalog_releases.is_empty()
            && !self.distro_loading;
        let query = self.catalog_search_query(cx);
        let distributions = self
            .distributions
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
            .take(self.catalog_visible)
            .map(|(index, distribution)| {
                let selected = self.selected_distribution == Some(index);
                div()
                    .id(("distribution", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .bg(rgb(if selected { 0x183932 } else { 0x0d151f }))
                    .border_1()
                    .border_color(rgb(if selected { 0x3ebfa7 } else { 0x1f2c3c }))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_distribution(index, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(22.))
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0x5bd7c0))
                                    .child(if distribution.rank == 0 {
                                        "·".into()
                                    } else {
                                        distribution.rank.to_string()
                                    }),
                            )
                            .child(
                                img(distribution.logo_url.clone())
                                    .size(px(24.))
                                    .object_fit(ObjectFit::Contain),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(distribution.name.clone()),
                                    )
                                    .child(div().text_xs().text_color(rgb(0x7890a8)).child(
                                        distribution.based_on.clone().unwrap_or_else(|| {
                                            if distribution.rank == 0 {
                                                "DistroWatch directory".into()
                                            } else {
                                                "Independent".into()
                                            }
                                        }),
                                    )),
                            ),
                    )
                    .child(div().text_xs().text_color(rgb(0x8fa4bd)).child(
                        if distribution.rank == 0 {
                            "Directory".into()
                        } else {
                            format!("{} / day", distribution.hits_per_day)
                        },
                    ))
            })
            .collect::<Vec<_>>();
        let releases = self
            .catalog_releases
            .iter()
            .enumerate()
            .take(30)
            .map(|(index, release)| {
                let selected = self.selected_release == Some(index);
                let size = release
                    .size
                    .map(format_bytes)
                    .unwrap_or_else(|| "Size unknown".into());
                let integrity = release
                    .checksum_algorithm
                    .filter(|_| release.checksum.is_some() || release.checksum_url.is_some())
                    .map(|algorithm| format!("Publisher {algorithm}"))
                    .unwrap_or_else(|| "No publisher checksum".into());
                div()
                    .id(("release", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .cursor_pointer()
                    .bg(rgb(if selected { 0x183932 } else { 0x0d151f }))
                    .border_1()
                    .border_color(rgb(if selected { 0x3ebfa7 } else { 0x1f2c3c }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_release = Some(index);
                        let has_checksum = this.catalog_releases.get(index).is_some_and(|release| {
                            release.checksum.is_some() || release.checksum_url.is_some()
                        });
                        this.status = if has_checksum {
                            "ISO selected • publisher checksum will be verified before use".into()
                        } else {
                            "ISO selected • publisher checksum unavailable; HTTPS length and boot structure will be checked".into()
                        };
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(release.name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .child(div().text_xs().text_color(rgb(0x8fa4bd)).child(size))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(if integrity.starts_with("Publisher") {
                                        0x5bd7c0
                                    } else {
                                        0x7890a8
                                    }))
                                    .child(integrity),
                            ),
                    )
            })
            .collect::<Vec<_>>();
        let distribution_state = if !query.is_empty() {
            &self.directory_state
        } else if self.quick_access == QuickAccess::Arch {
            &self.arch_state
        } else if self.quick_access == QuickAccess::Debian {
            &self.debian_state
        } else {
            &self.catalog_state
        };
        let distribution_message = if !query.is_empty()
            && matches!(
                distribution_state,
                CatalogState::Ready { .. } | CatalogState::Empty
            ) {
            "No matching distributions".into()
        } else {
            distribution_state.short_label("distributions")
        };
        let release_message = if self.selected_distribution.is_none()
            && matches!(self.details_state, CatalogState::Idle)
        {
            "Choose a distribution to resolve its current ISO files".into()
        } else {
            self.details_state.short_label("ISO releases")
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .border_1()
            .border_color(rgb(0x243244))
            .bg(rgb(0x111923))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(Icon::empty().path("ui/discover.svg"))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Discover distributions"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x7890a8))
                                            .child(
                                                "DistroWatch page-hit ranking measures interest—not quality or market share",
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        Button::new("reload-catalog")
                            .compact()
                            .icon(Icon::empty().path("ui/refresh.svg"))
                            .tooltip("Reload DistroWatch data")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.retry_discovery(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .when(layout.compact, |columns| columns.flex_col())
                    .child(
                        div()
                            .when(!layout.compact, |column| {
                                column
                                    .w(relative(if layout.wide { 0.28 } else { 0.34 }))
                                    .flex_shrink_0()
                            })
                            .when(layout.compact, |column| column.w_full())
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x8fa4bd))
                                    .child("POPULAR · SIX MONTHS"),
                            )
                            .child(
                                div()
                                    .h(layout.distribution_height)
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .overflow_y_scrollbar()
                                    .on_scroll_wheel(cx.listener(|this, event, _, cx| {
                                        this.load_more_discovery(event, cx)
                                    }))
                                    .when(distributions.is_empty(), |panel| {
                                        panel.child(
                                            div()
                                                .p_4()
                                                .rounded_lg()
                                                .bg(rgb(0x0d151f))
                                                .text_sm()
                                                .text_color(rgb(0x7890a8))
                                                .child(distribution_message),
                                        )
                                    })
                                    .children(distributions),
                            )
                            .when_some(self.selected_details.as_ref(), |column, details| {
                                column.when_some(details.screenshot_url.as_ref(), |column, url| {
                                    column.child(
                                        div()
                                            .relative()
                                            .h(layout.screenshot_height)
                                            .w_full()
                                            .overflow_hidden()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(rgb(0x243244))
                                            .child(
                                                div()
                                                    .size_full()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .gap_2()
                                                    .bg(rgb(0x0d151f))
                                                    .text_xs()
                                                    .text_color(rgb(0x6f8299))
                                                    .child(
                                                        Icon::empty()
                                                            .path("ui/image.svg")
                                                            .size(px(18.)),
                                                    )
                                                    .child(format!("{} screenshot", details.name)),
                                            )
                                            .child(
                                                img(url.clone())
                                                    .absolute()
                                                    .size_full()
                                                    .object_fit(ObjectFit::Contain),
                                            ),
                                    )
                                })
                            }),
                    )
                    .child(
                        div()
                            .when(!layout.compact, |column| column.flex_1().min_w(px(0.)))
                            .when(layout.compact, |column| column.w_full())
                            .flex()
                            .flex_col()
                            .gap_2()
                            .when_some(self.selected_details.as_ref(), |panel, details| {
                                panel
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_3()
                                            .when_some(details.logo_url.as_ref(), |row, url| {
                                                row.child(
                                                    img(url.clone())
                                                        .size(px(48.))
                                                        .object_fit(ObjectFit::Contain),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .text_base()
                                                            .font_weight(FontWeight::BOLD)
                                                            .child(details.name.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(rgb(0x8fa4bd))
                                                            .child(format!(
                                                                "{} · {} · {}",
                                                                details
                                                                    .os_type
                                                                    .as_deref()
                                                                    .unwrap_or("Unknown OS"),
                                                                details
                                                                    .origin
                                                                    .as_deref()
                                                                    .unwrap_or("Unknown origin"),
                                                                details
                                                                    .status
                                                                    .as_deref()
                                                                    .unwrap_or("Unknown status")
                                                            )),
                                                    )
                                                    .when_some(
                                                        details.visitor_rating.as_ref(),
                                                        |row, rating| {
                                                            row.child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(rgb(0x5bd7c0))
                                                                    .child(format!(
                                                                        "★ {rating}/10 · {} reviews",
                                                                        details
                                                                            .visitor_review_count
                                                                            .unwrap_or_default()
                                                                    )),
                                                            )
                                                        },
                                                    ),
                                            ),
                                    )
                                    .when_some(details.description.as_ref(), |panel, description| {
                                        panel.child(
                                            div()
                                                .max_h(px(62.))
                                                .overflow_hidden()
                                                .text_xs()
                                                .text_color(rgb(0xa9b8c9))
                                                .child(description.clone()),
                                        )
                                    })
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x7890a8))
                                            .child(format!(
                                                "Architecture: {}  ·  Desktop: {}",
                                                compact_list(&details.architectures, 4),
                                                compact_list(&details.desktops, 4)
                                            )),
                                    )
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(0x8fa4bd))
                                            .child("DIRECT ISO FILES"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x6f8299))
                                            .child(if self.distro_loading {
                                                "Loading…".into()
                                            } else {
                                                format!("{} found", self.catalog_releases.len())
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .h(layout.release_height)
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .overflow_y_scrollbar()
                                    .when(releases.is_empty(), |panel| {
                                        panel.child(
                                            div()
                                                .p_4()
                                                .rounded_lg()
                                                .bg(rgb(0x0d151f))
                                                .text_sm()
                                                .text_color(rgb(0x7890a8))
                                                .child(release_message),
                                        )
                                    })
                                    .children(releases),
                            )
                            .when(show_page_fallback, |panel| {
                                panel.child(
                                    Button::new("open-distrowatch-page")
                                        .primary()
                                        .icon(Icon::empty().path("ui/discover.svg"))
                                        .label("Open DistroWatch download page")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.open_selected_distrowatch_page(cx)
                                        })),
                                )
                            })
                            .when(!show_page_fallback, |panel| {
                                panel.child(
                                    Button::new("download-catalog-iso")
                                        .primary()
                                        .disabled(self.selected_release.is_none())
                                        .icon(Icon::empty().path("ui/download.svg"))
                                        .label("Download & use ISO")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.download_catalog_release(cx)
                                        })),
                                )
                            }),
                    ),
            )
    }

    fn catalog_card(&self, cx: &mut Context<Self>, layout: ViewportLayout) -> impl IntoElement {
        let advanced_label = if self.advanced {
            "Hide options"
        } else {
            "Setup options"
        };
        let content = if self.quick_access == QuickAccess::Windows {
            self.windows_quick_card(cx).into_any_element()
        } else if self.quick_access == QuickAccess::Omarchy {
            self.omarchy_quick_card(cx, layout).into_any_element()
        } else {
            match self.discovery_source {
                DiscoverySource::DistroWatch => {
                    self.distrowatch_catalog_card(cx, layout).into_any_element()
                }
                DiscoverySource::RaspberryPi => self.pi_catalog_card(cx).into_any_element(),
            }
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(layout.compact, |toolbar| toolbar.flex_col().items_start())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .flex_shrink_0()
                            .child(Icon::empty().path("ui/discover.svg"))
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::BOLD)
                                    .child("Discover"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .gap_2()
                            .child(
                                Button::new("quick-all")
                                    .compact()
                                    .when(
                                        self.quick_access == QuickAccess::All
                                            && self.discovery_source
                                                == DiscoverySource::DistroWatch,
                                        |button| button.primary(),
                                    )
                                    .label("All")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_quick_access(QuickAccess::All, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("quick-arch")
                                    .compact()
                                    .when(self.quick_access == QuickAccess::Arch, |button| {
                                        button.primary()
                                    })
                                    .label("Arch")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_quick_access(QuickAccess::Arch, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("quick-debian")
                                    .compact()
                                    .when(self.quick_access == QuickAccess::Debian, |button| {
                                        button.primary()
                                    })
                                    .label("Debian")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_quick_access(QuickAccess::Debian, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("quick-omarchy")
                                    .compact()
                                    .when(self.quick_access == QuickAccess::Omarchy, |button| {
                                        button.primary()
                                    })
                                    .label("Omarchy")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_quick_access(QuickAccess::Omarchy, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("quick-windows")
                                    .compact()
                                    .when(self.quick_access == QuickAccess::Windows, |button| {
                                        button.primary()
                                    })
                                    .label("Windows")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_quick_access(QuickAccess::Windows, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("quick-raspberry-pi")
                                    .compact()
                                    .when(
                                        self.discovery_source == DiscoverySource::RaspberryPi,
                                        |button| button.primary(),
                                    )
                                    .label("Raspberry Pi")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.show_raspberry_pi(cx)),
                                    ),
                            ),
                    )
                    .when(self.quick_access != QuickAccess::Windows, |toolbar| {
                        toolbar.child(
                            div()
                                .flex_1()
                                .min_w(px(220.))
                                .when(layout.compact, |search| search.w_full())
                                .child(Input::new(&self.catalog_search).w_full()),
                        )
                    })
                    .when(layout.wide, |toolbar| {
                        toolbar.child(
                            div()
                                .flex()
                                .items_center()
                                .flex_shrink_0()
                                .gap_2()
                                .child(
                                    Button::new("downloads")
                                        .compact()
                                        .icon(Icon::empty().path("ui/download.svg"))
                                        .label(format!("Downloads · {}", self.download_jobs.len()))
                                        .when(self.downloads_open, |button| button.primary())
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.toggle_downloads(cx)),
                                        ),
                                )
                                .child(
                                    Button::new("catalog")
                                        .compact()
                                        .icon(Icon::empty().path("ui/discover.svg"))
                                        .label("Close catalog")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.toggle_catalog(cx)),
                                        ),
                                )
                                .when(self.image.is_some(), |actions| {
                                    actions.child(
                                        Button::new("advanced")
                                            .compact()
                                            .icon(Icon::empty().path("ui/settings.svg"))
                                            .label(advanced_label)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.toggle_advanced(cx)
                                            })),
                                    )
                                })
                                .child(
                                    Button::new("refresh")
                                        .compact()
                                        .icon(Icon::empty().path("ui/refresh.svg"))
                                        .tooltip("Refresh removable drives")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.refresh_devices(cx)),
                                        ),
                                ),
                        )
                    }),
            )
            .child(content)
    }

    fn omarchy_quick_card(
        &self,
        cx: &mut Context<Self>,
        layout: ViewportLayout,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.distrowatch_catalog_card(cx, layout))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(0x3f3520))
                    .bg(rgb(0x18150f))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xe2ba68))
                                    .child("Omarchy MX Mac · Apple Silicon derivative"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xb6a17a))
                                    .child("Installs onto an existing Asahi Arch Minimal system; its releases contain signed installer files, not an ISO/IMG."),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x85775f))
                                    .child("github.com/maralcbr/omarchy-mx-mac · M1/M2/M3 · internal SSD workflow"),
                            ),
                    )
                    .child(
                        Button::new("omarchy-mx-mac-install-method")
                            .disabled(true)
                            .label("Not writable to USB"),
                    ),
            )
    }

    fn windows_quick_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let windows_image = self
            .image
            .as_ref()
            .is_some_and(|image| matches!(image.kind, ImageKind::WindowsInstaller { .. }));
        let supported_features = [
            "Standard Windows installation",
            "GPT or MBR + UEFI FAT32 media",
            "Split WIM files above 4 GiB",
            "Remove TPM / Secure Boot / RAM requirements",
            "Remove online Microsoft-account requirement",
            "Disable data collection / skip privacy questions",
            "Disable automatic BitLocker device encryption",
            "Create a named local administrator account",
            "Copy host locale and time zone",
            "QoL policies for bundled Windows experiences",
            "Windows CA 2023 signed bootloaders",
            "Apply SkuSiPolicy.p7b revocations",
            "Force Windows S Mode",
            "MD5 / SHA-1 / SHA-256 / SHA-512 checksums",
            "1 / 2 / 4-pass bad-block testing",
            "Reviewed erase phrase and removable-drive safety",
        ];
        let remaining_features = [
            "Windows To Go and internal-disk isolation",
            "Legacy BIOS boot and NTFS / UEFI:NTFS media",
            "Fully unattended silent disk installation",
        ];
        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .rounded_xl()
            .border_1()
            .border_color(rgb(0x243244))
            .bg(rgb(0x111923))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::BOLD)
                                    .child("Windows installer media"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x7890a8))
                                    .child("Complete Rufus 4.15 Windows inventory · working controls are clearly separated"),
                            ),
                    )
                    .child(
                        Button::new("windows-choose-iso")
                            .primary()
                            .icon(Icon::empty().path("ui/image.svg"))
                            .label(if windows_image {
                                "Replace Windows ISO"
                            } else {
                                "Choose Windows ISO"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.choose_image(cx))),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(3)
                    .gap_2()
                    .children(supported_features.into_iter().map(|feature| {
                        div()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(0x243244))
                            .bg(rgb(0x0d151f))
                            .text_xs()
                            .text_color(rgb(0xa9b8c9))
                            .child(format!("✓  {feature}"))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(0x3a3022))
                    .bg(rgb(0x18150f))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xd8b46b))
                            .child("Rufus Windows features still being implemented"),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(3)
                            .gap_2()
                            .children(remaining_features.into_iter().map(|feature| {
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xa99370))
                                    .child(format!("○  {feature}"))
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x806f55))
                            .child("Unavailable items are intentionally not clickable. Silent installation can erase the first disk Windows Setup detects and requires a separate high-friction safety design."),
                    ),
            )
            .when(windows_image, |panel| panel.child(self.advanced_card(cx)))
            .when(!windows_image, |panel| {
                panel.child(
                    div()
                        .p_4()
                        .rounded_lg()
                        .bg(rgb(0x0f1925))
                        .text_sm()
                        .text_color(rgb(0x8fa4bd))
                        .child("Choose an inspected Windows installer ISO to reveal the independent Windows setup checkboxes. No Windows option is applied silently."),
                )
            })
    }

    fn pi_catalog_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.catalog_search_query(cx);
        let devices = self
            .pi_catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .devices
                    .iter()
                    .enumerate()
                    .map(|(index, device)| {
                        let selected = self.selected_pi_device == Some(index);
                        div()
                            .id(("pi-device", index))
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .cursor_pointer()
                            .bg(rgb(if selected { 0x183932 } else { 0x0d151f }))
                            .border_1()
                            .border_color(rgb(if selected { 0x3ebfa7 } else { 0x1f2c3c }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_pi_device(Some(index), cx)
                            }))
                            .when_some(device.icon_url.as_ref(), |row, icon| {
                                row.child(
                                    img(icon.clone())
                                        .size(px(32.))
                                        .object_fit(ObjectFit::Contain),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(device.name.clone()),
                                    )
                                    .when_some(
                                        device.description.as_ref(),
                                        |column, description| {
                                            column.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x7890a8))
                                                    .child(description.clone()),
                                            )
                                        },
                                    ),
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let images = self
            .visible_pi_images()
            .into_iter()
            .filter(|(_, image)| {
                query.is_empty()
                    || image.name.to_lowercase().contains(&query)
                    || image
                        .description
                        .as_deref()
                        .is_some_and(|value| value.to_lowercase().contains(&query))
                    || image
                        .category
                        .as_deref()
                        .is_some_and(|value| value.to_lowercase().contains(&query))
                    || image.archive_name.to_lowercase().contains(&query)
            })
            .take(self.pi_visible)
            .map(|(index, image)| {
                let selected = self.selected_pi_image == Some(index);
                div()
                    .id(("pi-image", index))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .cursor_pointer()
                    .bg(rgb(if selected { 0x183932 } else { 0x0d151f }))
                    .border_1()
                    .border_color(rgb(if selected { 0x3ebfa7 } else { 0x1f2c3c }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_pi_image = Some(index);
                        this.status =
                            "Raspberry Pi image selected • download will be extracted and verified"
                                .into();
                        cx.notify();
                    }))
                    .when_some(image.icon_url.as_ref(), |row, icon| {
                        row.child(
                            img(icon.clone())
                                .size(px(32.))
                                .object_fit(ObjectFit::Contain),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(image.name.clone()),
                            )
                            .child(div().text_xs().text_color(rgb(0x7890a8)).child(format!(
                                "{} · {}",
                                image.release_date.as_deref().unwrap_or("Date unknown"),
                                image.download_size.map(format_bytes).unwrap_or_default()
                            ))),
                    )
            })
            .collect::<Vec<_>>();
        let selected = self
            .selected_pi_image
            .and_then(|index| self.pi_catalog.as_ref()?.images.get(index));
        let pi_message = self.pi_state.short_label("Raspberry Pi images");
        let image_message = if self.pi_catalog.is_some() {
            "No compatible image found".into()
        } else {
            pi_message.clone()
        };

        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .rounded_xl()
            .border_1()
            .border_color(rgb(0x243244))
            .bg(rgb(0x111923))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Official Raspberry Pi Imager catalog"),
                            )
                            .child(div().text_xs().text_color(rgb(0x7890a8)).child(
                                "Board compatibility, compressed and extracted checksums included",
                            )),
                    )
                    .child(
                        Button::new("reload-pi-catalog")
                            .compact()
                            .icon(Icon::empty().path("ui/refresh.svg"))
                            .tooltip("Reload Raspberry Pi catalog")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_raspberry_pi_with(CacheMode::Refresh, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(
                        div()
                            .w(px(230.))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x8fa4bd))
                                    .child("BOARD FILTER"),
                            )
                            .child(
                                Button::new("pi-device-all")
                                    .when(self.selected_pi_device.is_none(), |button| {
                                        button.primary()
                                    })
                                    .label("All images")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.select_pi_device(None, cx)
                                    })),
                            )
                            .child(
                                div()
                                    .h(px(390.))
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .overflow_y_scrollbar()
                                    .on_scroll_wheel(cx.listener(|this, event, _, cx| {
                                        this.load_more_discovery(event, cx)
                                    }))
                                    .when(devices.is_empty(), |panel| {
                                        panel.child(
                                            div()
                                                .p_4()
                                                .text_sm()
                                                .text_color(rgb(0x7890a8))
                                                .child(pi_message),
                                        )
                                    })
                                    .children(devices),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0x8fa4bd))
                                    .child("COMPATIBLE IMAGES"),
                            )
                            .child(
                                div()
                                    .h(px(430.))
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .overflow_y_scrollbar()
                                    .when(images.is_empty(), |panel| {
                                        panel.child(
                                            div()
                                                .p_4()
                                                .text_sm()
                                                .text_color(rgb(0x7890a8))
                                                .child(image_message),
                                        )
                                    })
                                    .children(images),
                            ),
                    )
                    .child(
                        div()
                            .w(px(290.))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .when_some(selected, |panel, image| {
                                panel
                                    .when_some(image.icon_url.as_ref(), |panel, icon| {
                                        panel.child(
                                            img(icon.clone())
                                                .w_full()
                                                .h(px(90.))
                                                .object_fit(ObjectFit::Contain),
                                        )
                                    })
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(FontWeight::BOLD)
                                            .child(image.name.clone()),
                                    )
                                    .when_some(image.description.as_ref(), |panel, description| {
                                        panel.child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(0xa9b8c9))
                                                .child(description.clone()),
                                        )
                                    })
                                    .child(div().text_xs().text_color(rgb(0x7890a8)).child(
                                        format!(
                                            "Download {} · Expanded {}\nReleased {}\n{}",
                                            image
                                                .download_size
                                                .map(format_bytes)
                                                .unwrap_or_default(),
                                            image
                                                .extracted_size
                                                .map(format_bytes)
                                                .unwrap_or_default(),
                                            image.release_date.as_deref().unwrap_or("unknown"),
                                            image
                                                .category
                                                .as_deref()
                                                .unwrap_or("Raspberry Pi image")
                                        ),
                                    ))
                            })
                            .child(
                                Button::new("download-pi-image")
                                    .primary()
                                    .disabled(selected.is_none())
                                    .icon(Icon::empty().path("ui/download.svg"))
                                    .label("Download, verify & use")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.download_pi_catalog_image(cx)
                                    })),
                            ),
                    ),
            )
    }

    fn source_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let details = self
            .image
            .as_ref()
            .map(|image| {
                format!(
                    "{}\n{}  •  {}",
                    image.path.display(),
                    image.kind,
                    format_bytes(image.size)
                )
            })
            .unwrap_or_else(|| "ISO, IMG, RAW, or compressed disk image".into());
        div()
            .flex()
            .flex_1()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .border_1()
            .border_color(rgb(0x243244))
            .bg(rgb(0x111923))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(28.))
                            .rounded_full()
                            .bg(rgb(0x183932))
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0x5bd7c0))
                            .child("1"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xe8f0f8))
                            .child(Icon::empty().path("ui/image.svg"))
                            .child("Choose an image"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(0x0d151f))
                    .border_1()
                    .border_color(rgb(0x1f2c3c))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(div().text_sm().text_color(rgb(0xa9b8c9)).child(details))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x6f8299))
                                    .child("The image is inspected before any write is allowed"),
                            ),
                    )
                    .child(
                        Button::new("choose-image")
                            .primary()
                            .disabled(self.image_loading)
                            .icon(Icon::empty().path("ui/image.svg"))
                            .label(if self.image_loading {
                                "Inspecting…"
                            } else if self.image.is_some() {
                                "Change"
                            } else {
                                "Browse"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.choose_image(cx))),
                    ),
            )
    }

    fn advanced_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let windows_available = self
            .image
            .as_ref()
            .is_some_and(|image| matches!(image.kind, ImageKind::WindowsInstaller { .. }));
        let selected_count = [
            self.options.windows.bypass_hardware_requirements,
            self.options.windows.allow_offline_account,
            self.options.windows.local_account.is_some(),
            self.options.windows.regional.is_some(),
            self.options.windows.minimize_data_collection,
            self.options.windows.disable_bitlocker,
            self.options.windows.quality_of_life,
            self.options.windows.use_windows_ca_2023,
            self.options.windows.apply_skusi_policy,
            self.options.windows.force_s_mode,
        ]
        .into_iter()
        .filter(|selected| *selected)
        .count();

        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .border_1()
            .border_color(rgb(0x243244))
            .bg(rgb(0x111923))
            .when(windows_available, |panel| {
                panel
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(Icon::empty().path("ui/settings.svg"))
                                    .child("Windows installer options"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x7890a8))
                                    .child(format!("{selected_count} selected")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x7890a8))
                                    .child("Partition scheme · target firmware"),
                            )
                            .child(Select::new(&self.windows_partition_scheme).w_full()),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(2)
                            .gap_3()
                            .child(
                                Checkbox::new("windows-requirements")
                                    .checked(self.options.windows.bypass_hardware_requirements)
                                    .label("Bypass TPM, Secure Boot and RAM checks")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.options.windows.bypass_hardware_requirements =
                                            *checked;
                                        this.status = "Windows installer selections updated".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new("windows-offline-account")
                                    .checked(self.options.windows.allow_offline_account)
                                    .label("Expose local/offline account setup")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.options.windows.allow_offline_account = *checked;
                                        this.status = "Windows installer selections updated".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new("windows-local-account")
                                    .checked(self.options.windows.local_account.is_some())
                                    .label(format!(
                                        "Create local account: {}",
                                        self.options
                                            .windows
                                            .local_account
                                            .clone()
                                            .or_else(bootable_core::suggested_account_name)
                                            .unwrap_or_else(|| "User".into())
                                    ))
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.options.windows.local_account = checked.then(|| {
                                            bootable_core::suggested_account_name()
                                                .unwrap_or_else(|| "User".into())
                                        });
                                        this.status = "Windows installer selections updated".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new("windows-regional")
                                    .checked(self.options.windows.regional.is_some())
                                    .label("Copy this computer's locale and time zone")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.options.windows.regional = checked
                                            .then(bootable_core::host_regional_options);
                                        this.status = "Windows installer selections updated".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new("windows-privacy")
                                    .checked(self.options.windows.minimize_data_collection)
                                    .label("Apply privacy-focused OOBE defaults")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.options.windows.minimize_data_collection = *checked;
                                        this.status = "Windows installer selections updated".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new("windows-bitlocker")
                                    .checked(self.options.windows.disable_bitlocker)
                                    .label("Disable automatic BitLocker encryption")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.options.windows.disable_bitlocker = *checked;
                                        this.status = "Windows installer selections updated".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new("windows-qol")
                                    .checked(self.options.windows.quality_of_life)
                                    .label("QoL: reduce Copilot, OneDrive, Teams, suggestions, and Fast Startup")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.options.windows.quality_of_life = *checked;
                                        this.status = "Windows installer selections updated".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new("windows-ca-2023")
                                    .checked(self.options.windows.use_windows_ca_2023)
                                    .label("Use Windows UEFI CA 2023 signed bootloaders")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.options.windows.use_windows_ca_2023 = *checked;
                                        this.status = "CA 2023 media requires updated Secure Boot firmware certificates".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new("windows-skusi-policy")
                                    .checked(self.options.windows.apply_skusi_policy)
                                    .label("Apply SkuSiPolicy.p7b Secure Boot revocations")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.options.windows.apply_skusi_policy = *checked;
                                        this.status = "Windows installer selections updated".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new("windows-s-mode")
                                    .checked(self.options.windows.force_s_mode)
                                    .label("Force Windows S Mode (expert)")
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.options.windows.force_s_mode = *checked;
                                        this.status = "S Mode may remain enforced after reinstall; review before writing".into();
                                        cx.notify();
                                    })),
                            ),
                    )
            })
            .when(!windows_available, |panel| {
                panel
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(Icon::empty().path("ui/settings.svg"))
                            .child("Linux / Unix boot media"),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(2)
                            .gap_3()
                            .child(
                                Checkbox::new("raw-image-write")
                                    .checked(true)
                                    .disabled(true)
                                    .label("Preserve the complete bootable disk layout"),
                            )
                            .child(
                                Checkbox::new("raw-image-verify")
                                    .checked(true)
                                    .disabled(true)
                                    .label("Verify the written bytes with SHA-256"),
                            ),
                    )
            })
            .child(div().h(px(1.)).bg(rgb(0x243244)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Media tools"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x7890a8))
                                    .child("Verification and backup utilities"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                Button::new("bad-block-check")
                                    .compact()
                                    .label(format!("Bad blocks · {}", self.options.bad_block_check))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cycle_bad_blocks(cx)),
                                    ),
                            )
                            .child(
                                Button::new("checksum-algorithm")
                                    .compact()
                                    .icon(Icon::empty().path("ui/hash.svg"))
                                    .label(self.checksum_algorithm.to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cycle_checksum_algorithm(cx)
                                    })),
                            )
                            .child(
                                Button::new("checksum-image")
                                    .compact()
                                    .icon(Icon::empty().path("ui/hash.svg"))
                                    .label("Verify image")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.checksum_image(cx)),
                                    ),
                            )
                            .child(
                                Button::new("choose-folder")
                                    .compact()
                                    .icon(Icon::empty().path("ui/folder.svg"))
                                    .label("Image folder")
                                    .on_click(cx.listener(|this, _, _, cx| this.choose_folder(cx))),
                            )
                            .child(
                                Button::new("backup-device")
                                    .compact()
                                    .icon(Icon::empty().path("ui/backup.svg"))
                                    .label("Back up drive")
                                    .on_click(cx.listener(|this, _, _, cx| this.backup_device(cx))),
                            ),
                    ),
            )
    }

    fn target_cards(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let cards = self
            .devices
            .iter()
            .enumerate()
            .map(|(index, device)| {
                let selected = self.selected_device == Some(index);
                let blocked = !device.is_eligible_target();
                let border = if selected { 0x36d3b4 } else { 0x283345 };
                let status = if device.system_disk {
                    "System disk • blocked"
                } else if !device.removable {
                    "Internal disk • blocked"
                } else if device.read_only {
                    "Read-only • blocked"
                } else {
                    "Removable • eligible"
                };
                div()
                    .id(("device", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(border))
                    .bg(rgb(if selected { 0x15302f } else { 0x111923 }))
                    .when(!blocked, |element| {
                        element
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_device = Some(index);
                                this.status =
                                    "Target selected; preview the plan before writing".into();
                                cx.notify();
                            }))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0xe8f0f8))
                                    .child(device.display_name()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(if blocked { 0xf29a9a } else { 0x8fa4bd }))
                                    .child(format!("{}  •  {status}", device.path.display())),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0x9ec9c1))
                            .child(format_bytes(device.capacity)),
                    )
            })
            .collect::<Vec<_>>();

        div().flex().flex_col().gap_3().children(cards)
    }

    fn download_history_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .download_jobs
            .iter()
            .take(20)
            .enumerate()
            .map(|(index, job)| {
                let id = job.id.clone();
                let retry_id = id.clone();
                let use_id = id.clone();
                let remove_id = id.clone();
                let status_color = match job.status {
                    DownloadStatus::Completed => 0x5bd7c0,
                    DownloadStatus::Failed | DownloadStatus::Cancelled => 0xf29a9a,
                    DownloadStatus::Interrupted | DownloadStatus::Paused => 0xe5b95f,
                    DownloadStatus::Queued | DownloadStatus::Running => 0x8fc7ff,
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(0x243244))
                    .bg(rgb(0x0d151f))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w(px(0.))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(job.label.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x7890a8))
                                            .child(job.destination.display().to_string()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(status_color))
                                            .child(job.status.to_string()),
                                    )
                                    .when(job.status.can_retry(), |actions| {
                                        actions.child(
                                            Button::new(("retry-download", index))
                                                .compact()
                                                .label("Retry")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.retry_managed_download(
                                                        retry_id.clone(),
                                                        cx,
                                                    )
                                                })),
                                        )
                                    })
                                    .when(job.status == DownloadStatus::Completed, |actions| {
                                        actions.child(
                                            Button::new(("use-download", index))
                                                .compact()
                                                .primary()
                                                .label("Use")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.use_managed_download(&use_id, cx)
                                                })),
                                        )
                                    })
                                    .when(
                                        !matches!(
                                            job.status,
                                            DownloadStatus::Running | DownloadStatus::Paused
                                        ),
                                        |actions| {
                                            actions.child(
                                                Button::new(("remove-download", index))
                                                    .compact()
                                                    .label("Remove")
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.remove_managed_download(
                                                                &remove_id, cx,
                                                            )
                                                        },
                                                    )),
                                            )
                                        },
                                    ),
                            ),
                    )
                    .when_some(job.progress_ratio(), |row, ratio| {
                        row.child(
                            div()
                                .w_full()
                                .h(px(3.))
                                .rounded_full()
                                .bg(rgb(0x243244))
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(ratio as f32))
                                        .rounded_full()
                                        .bg(rgb(status_color)),
                                ),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x8fa4bd))
                            .child(job.error.clone().unwrap_or_else(|| job.message.clone())),
                    )
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .border_1()
            .border_color(rgb(0x243244))
            .bg(rgb(0x111923))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(Icon::empty().path("ui/download.svg"))
                            .child("Downloads"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x7890a8))
                            .child("Persistent history · interrupted transfers can resume"),
                    ),
            )
            .child(
                div()
                    .max_h(px(260.))
                    .overflow_y_scrollbar()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(rows.is_empty(), |list| {
                        list.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(rgb(0x7890a8))
                                .child("No managed downloads yet"),
                        )
                    })
                    .children(rows),
            )
    }

    fn header_bar(&self, cx: &mut Context<Self>, compact: bool) -> impl IntoElement {
        let advanced_label = if self.advanced {
            "Hide options"
        } else {
            "Setup options"
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .when(compact, |header| {
                header.flex_col().items_start().justify_start().gap_2()
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        img("brand/bootable-mark.svg")
                            .size(px(38.))
                            .object_fit(ObjectFit::Contain),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::BOLD)
                                            .child("BOOTABLE"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0xe5b95f))
                                            .child("α"),
                                    ),
                            )
                            .when(!compact, |brand| {
                                brand.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0x7890a8))
                                        .child("Boot media, written deliberately."),
                                )
                            }),
                    )
                    .when(!compact, |brand| {
                        brand
                            .child(div().w(px(1.)).h(px(34.)).bg(rgb(0x243244)))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(
                                        if self.review_plan.is_some() {
                                            "Review write plan"
                                        } else {
                                            "Create boot media"
                                        },
                                    ))
                                    .child(div().text_xs().text_color(rgb(0x8fa4bd)).child(
                                        if self.review_plan.is_some() {
                                            "Inspect every operation before confirmation."
                                        } else {
                                            "Image → removable drive → verified result"
                                        },
                                    )),
                            )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .when(compact, |actions| actions.w_full().flex_wrap())
                    .gap_2()
                    .child(
                        Button::new("downloads")
                            .icon(Icon::empty().path("ui/download.svg"))
                            .label(if compact {
                                format!("Jobs · {}", self.download_jobs.len())
                            } else {
                                format!("Downloads · {}", self.download_jobs.len())
                            })
                            .when(self.downloads_open, |button| button.primary())
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_downloads(cx))),
                    )
                    .child(
                        Button::new("catalog")
                            .icon(Icon::empty().path("ui/discover.svg"))
                            .label(if compact {
                                if self.catalog_open {
                                    "Catalog ×"
                                } else {
                                    "Discover"
                                }
                            } else if self.catalog_open {
                                "Close catalog"
                            } else {
                                "Discover"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_catalog(cx))),
                    )
                    .when(self.image.is_some(), |actions| {
                        actions.child(
                            Button::new("advanced")
                                .icon(Icon::empty().path("ui/settings.svg"))
                                .label(if compact { "Options" } else { advanced_label })
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_advanced(cx))),
                        )
                    })
                    .child(
                        Button::new("refresh")
                            .compact()
                            .icon(Icon::empty().path("ui/refresh.svg"))
                            .tooltip("Refresh removable drives")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_devices(cx))),
                    ),
            )
    }

    fn target_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .border_1()
            .border_color(rgb(0x243244))
            .bg(rgb(0x111923))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(28.))
                                    .rounded_full()
                                    .bg(rgb(0x183932))
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0x5bd7c0))
                                    .child("2"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(Icon::empty().path("ui/usb.svg"))
                                    .child("Choose a drive"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x6f8299))
                            .child("Auto-detecting removable media"),
                    ),
            )
            .child(
                div()
                    .h(px(104.))
                    .overflow_y_scrollbar()
                    .child(self.target_cards(cx)),
            )
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let readiness = self.review_readiness();
        let download_state = self.download_control.as_ref().map(OperationControl::state);
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .py_2()
            .rounded_xl()
            .border_1()
            .border_color(rgb(0x243244))
            .bg(rgb(0x0f1925))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.))
                            .rounded_full()
                            .bg(rgb(0x183932))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0x5bd7c0))
                            .child("3"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_sm()
                            .text_color(rgb(0xa9b8c9))
                            .child(self.status.clone())
                            .when_some(self.active_download.as_ref(), |column, progress| {
                                let ratio = progress
                                    .total
                                    .filter(|total| *total > 0)
                                    .map(|total| progress.completed as f32 / total as f32)
                                    .unwrap_or(0.)
                                    .clamp(0., 1.);
                                column.child(
                                    div()
                                        .w_full()
                                        .h(px(4.))
                                        .rounded_full()
                                        .bg(rgb(0x243244))
                                        .child(
                                            div()
                                                .h_full()
                                                .w(relative(ratio))
                                                .rounded_full()
                                                .bg(rgb(0x5bd7c0)),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(download_state, |actions, state| {
                        actions
                            .child(
                                Button::new("pause-download")
                                    .label(if state == OperationState::Paused {
                                        "Resume"
                                    } else {
                                        "Pause"
                                    })
                                    .disabled(state == OperationState::Cancelled)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_download_pause(cx)
                                        }),
                                    ),
                            )
                            .child(
                                Button::new("cancel-download")
                                    .label(if state == OperationState::Cancelled {
                                        "Cancelling…"
                                    } else {
                                        "Cancel"
                                    })
                                    .disabled(state == OperationState::Cancelled)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_download(cx)),
                                    ),
                            )
                    })
                    .when(download_state.is_none(), |actions| {
                        actions.child(
                            Button::new("preview")
                                .primary()
                                .icon(Icon::empty().path("ui/review.svg"))
                                .label(readiness.action_label())
                                .disabled(readiness != ReviewReadiness::Ready)
                                .on_click(cx.listener(|this, _, _, cx| this.preview_plan(cx))),
                        )
                    }),
            )
    }
}

impl Render for BootableView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let layout = ViewportLayout::new(viewport.width, viewport.height);
        div()
            .size_full()
            .relative()
            .bg(rgb(0x0b1119))
            .text_color(rgb(0xe8f0f8))
            .flex()
            .flex_col()
            .child(
                TitleBar::new()
                    .bg(rgb(0x0b1119))
                    .border_color(rgb(0x243244))
                    .text_color(rgb(0xe8f0f8))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_color(rgb(0x5bd7c0))
                            .child(
                                img("brand/bootable-mark.svg")
                                    .size(px(22.))
                                    .object_fit(ObjectFit::Contain),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xe8f0f8))
                                    .child("BOOTABLE"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xe5b95f))
                                    .child("α"),
                            ),
                    ),
            )
            .child(
                div().flex().flex_1().min_h(px(0.)).justify_center().child(
                    div()
                        .w_full()
                        .h_full()
                        .max_w(px(1_720.))
                        .flex()
                        .flex_col()
                        .when(
                            !(self.catalog_open && self.review_plan.is_none() && layout.wide),
                            |shell| {
                                shell.child(
                                    div()
                                        .flex_shrink_0()
                                        .px_5()
                                        .pt_4()
                                        .pb_3()
                                        .when(layout.compact, |header| header.px_3().pt_3().pb_2())
                                        .child(self.header_bar(cx, layout.compact)),
                                )
                            },
                        )
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_h(px(0.))
                                .overflow_y_scrollbar()
                                .px_5()
                                .when(layout.compact, |content| content.px_3())
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .flex_col()
                                        .gap_3()
                                        .when(self.review_plan.is_some(), |panel| {
                                            panel.child(self.review_card(cx))
                                        })
                                        .when(self.review_plan.is_none(), |panel| {
                                            panel
                                                .when(self.downloads_open, |panel| {
                                                    panel.child(self.download_history_card(cx))
                                                })
                                                .when(self.catalog_open, |panel| {
                                                    panel.child(self.catalog_card(cx, layout))
                                                })
                                                .child(
                                                    div()
                                                        .flex()
                                                        .gap_3()
                                                        .when(layout.compact, |chooser| {
                                                            chooser.flex_col()
                                                        })
                                                        .child(self.source_card(cx))
                                                        .child(self.target_panel(cx)),
                                                )
                                                .when(
                                                    self.image.is_some() && self.advanced,
                                                    |panel| panel.child(self.advanced_card(cx)),
                                                )
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .px_5()
                                .pt_3()
                                .pb_4()
                                .when(layout.compact, |footer| footer.px_3().pt_2().pb_3())
                                .when(self.review_plan.is_some(), |footer| {
                                    footer.child(self.review_footer(cx))
                                })
                                .when(self.review_plan.is_none(), |footer| {
                                    footer.child(self.status_bar(cx))
                                }),
                        ),
                ),
            )
            .when(self.confirm_modal, |root| {
                root.child(self.write_confirmation_modal(cx))
            })
    }
}

fn review_value(label: &'static str, value: String) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(rgb(0x243244))
        .bg(rgb(0x0d151f))
        .child(div().text_xs().text_color(rgb(0x8fa4bd)).child(label))
        .child(div().text_sm().child(value))
}

fn compact_list(values: &[String], limit: usize) -> String {
    if values.is_empty() {
        return "Not listed".into();
    }
    let mut result = values
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if values.len() > limit {
        result.push_str(&format!(" +{}", values.len() - limit));
    }
    result
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

fn main() {
    Application::new()
        .with_assets(BootableAssets)
        .run(|cx: &mut App| {
            if let Ok(client) = PreviewHttpClient::new() {
                cx.set_http_client(Arc::new(client));
            }
            gpui_component::init(cx);
            gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
            {
                let theme = gpui_component::Theme::global_mut(cx);
                theme.primary = rgb(0x5bd7c0).into();
                theme.primary_hover = rgb(0x73e3cd).into();
                theme.primary_active = rgb(0x43bea7).into();
                theme.primary_foreground = rgb(0x07130f).into();
                theme.accent = rgb(0x183932).into();
                theme.accent_foreground = rgb(0x7be8d2).into();
                theme.border = rgb(0x243244).into();
                theme.radius = px(8.);
                theme.radius_lg = px(12.);
            }
            let bounds = Bounds::centered(None, size(px(1080.), px(720.)), cx);
            cx.spawn(async move |cx| {
                let mut titlebar = TitleBar::title_bar_options();
                titlebar.title = Some("Bootable Alpha".into());
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(880.), px(640.))),
                    titlebar: Some(titlebar),
                    app_id: Some("app.bootable.Bootable".into()),
                    window_decorations: Some(WindowDecorations::Client),
                    ..WindowOptions::default()
                };
                cx.open_window(options, |window, cx| {
                    let view = cx.new(|cx| BootableView::new(window, cx));
                    let close_guard = view.downgrade();
                    window.on_window_should_close(cx, move |_, cx| {
                        let Some(view) = close_guard.upgrade() else {
                            return true;
                        };
                        let view_state = view.read(cx);
                        if !view_state.active_write && view_state.download_control.is_none() {
                            return true;
                        }
                        view.update(cx, |view, cx| {
                            if let Some(control) = &view.download_control {
                                control.cancel();
                                view.status = "Cancelling download safely • close again after temporary data is cleaned".into();
                            } else if let Some(control) = &view.write_control {
                                control.cancel();
                                view.status = "Stopping write safely • close again after completed writes are flushed".into();
                            } else {
                                view.status =
                                    "Writing is active • do not close the app or unplug the target"
                                        .into();
                            }
                            cx.notify();
                        });
                        false
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                })?;
                Ok::<_, anyhow::Error>(())
            })
            .detach();
        });
}

#[cfg(test)]
mod tests {
    use super::{BootableAssets, ViewportLayout};
    use gpui::{AssetSource, px};

    #[test]
    fn embeds_every_svg_asset() {
        let assets = BootableAssets;
        for path in [
            "brand/bootable-mark.svg",
            "brand/bootable-logo.svg",
            "ui/image.svg",
            "ui/usb.svg",
            "ui/settings.svg",
            "ui/refresh.svg",
            "ui/hash.svg",
            "ui/folder.svg",
            "ui/backup.svg",
            "ui/review.svg",
            "ui/discover.svg",
            "ui/download.svg",
            "icons/window-minimize.svg",
            "icons/window-maximize.svg",
            "icons/window-restore.svg",
            "icons/window-close.svg",
        ] {
            let data = assets.load(path).expect("asset load").expect("asset data");
            assert!(data.starts_with(b"<svg"), "invalid SVG at {path}");
        }
    }

    #[test]
    fn viewport_layout_uses_space_without_breaking_compact_windows() {
        let compact = ViewportLayout::new(px(800.), px(1_400.));
        assert!(compact.compact);
        assert!(!compact.wide);
        assert_eq!(compact.distribution_height, px(288.));

        let regular = ViewportLayout::new(px(1_200.), px(900.));
        assert!(!regular.compact);
        assert!(!regular.wide);
        assert_eq!(regular.distribution_height, px(288.));

        let ultrawide_height = ViewportLayout::new(px(2_048.), px(1_120.));
        assert!(!ultrawide_height.compact);
        assert!(ultrawide_height.wide);
        assert_eq!(ultrawide_height.distribution_height, px(288.));
        assert_eq!(ultrawide_height.screenshot_height, px(104.));
        assert_eq!(ultrawide_height.release_height, px(136.));

        let tall = ViewportLayout::new(px(1_200.), px(1_500.));
        assert_eq!(tall.distribution_height, px(288.));
        assert_eq!(tall.screenshot_height, px(112.));
        assert_eq!(tall.release_height, px(144.));
    }
}
