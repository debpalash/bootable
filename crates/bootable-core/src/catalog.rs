use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::redirect::Policy;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::checksum;
use crate::download;
use crate::error::{Error, Result, io_error};
use crate::operation::ensure_workspace;
use crate::{ChecksumAlgorithm, OperationControl, Progress, ProgressPhase};

const DISTROWATCH_BASE: &str = "https://distrowatch.com/";
const POPULARITY_URL: &str = "https://distrowatch.com/dwres.php?resource=popularity";
const SEARCH_URL: &str = "https://distrowatch.com/search.php";
// DistroWatch rejects generic application-style user agents even for interactive,
// human-triggered requests. Use a conventional desktop browser signature while
// retaining Bootable's existing cache and request limits.
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";
const MAX_CATALOG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ARTWORK_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CRAWL_DEPTH: usize = 3;
const MAX_CRAWL_PAGES: usize = 16;
const MAX_CHECKSUM_DIRECTORY_PAGES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistributionSummary {
    pub rank: u32,
    pub name: String,
    pub slug: String,
    pub hits_per_day: u32,
    pub based_on: Option<String>,
    pub page_url: String,
    pub logo_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistributionDetails {
    pub name: String,
    pub slug: String,
    pub page_url: String,
    pub home_page: Option<String>,
    pub release_date: Option<String>,
    pub image_size: Option<String>,
    pub download_pages: Vec<String>,
    pub logo_url: Option<String>,
    pub screenshot_url: Option<String>,
    pub gallery_url: Option<String>,
    pub description: Option<String>,
    pub os_type: Option<String>,
    pub based_on: Option<String>,
    pub origin: Option<String>,
    pub architectures: Vec<String>,
    pub desktops: Vec<String>,
    pub categories: Vec<String>,
    pub status: Option<String>,
    pub last_update: Option<String>,
    pub visitor_rating: Option<String>,
    pub visitor_review_count: Option<u32>,
    pub documentation_pages: Vec<String>,
    pub screenshot_pages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsoRelease {
    pub name: String,
    pub url: String,
    pub size: Option<u64>,
    pub published: Option<String>,
    pub checksum_algorithm: Option<ChecksumAlgorithm>,
    pub checksum: Option<String>,
    pub checksum_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistributionBundle {
    pub details: DistributionDetails,
    pub releases: Vec<IsoRelease>,
    pub warnings: Vec<String>,
}

pub(crate) fn popular_distributions(limit: usize) -> Result<Vec<DistributionSummary>> {
    let html = fetch_text(POPULARITY_URL)?;
    parse_popularity(&html, limit)
}

pub(crate) fn distribution_directory() -> Result<Vec<DistributionSummary>> {
    parse_distribution_directory(&fetch_text(SEARCH_URL)?)
}

pub(crate) fn distributions_based_on(base: &str) -> Result<Vec<DistributionSummary>> {
    if !matches!(base, "Arch" | "Debian") {
        return Err(Error::InvalidCatalog(
            "quick base search supports Arch or Debian".into(),
        ));
    }
    let mut url = Url::parse(SEARCH_URL)
        .map_err(|error| Error::InvalidCatalog(format!("invalid search URL: {error}")))?;
    url.query_pairs_mut()
        .append_pair("ostype", "All")
        .append_pair("category", "All")
        .append_pair("origin", "All")
        .append_pair("basedon", base)
        .append_pair("notbasedon", "None")
        .append_pair("desktop", "All")
        .append_pair("architecture", "All")
        .append_pair("package", "All")
        .append_pair("rolling", "All")
        .append_pair("isosize", "All")
        .append_pair("netinstall", "All")
        .append_pair("language", "All")
        .append_pair("defaultinit", "All")
        .append_pair("status", "Active");
    let mut entries = parse_base_search(&fetch_text(url.as_str())?, base)?;
    let popularity = popular_distributions(usize::MAX)?;
    rank_base_results(&mut entries, &popularity);
    Ok(entries)
}

fn rank_base_results(entries: &mut [DistributionSummary], popularity: &[DistributionSummary]) {
    let ranking = popularity
        .iter()
        .map(|entry| (entry.slug.as_str(), (entry.rank, entry.hits_per_day)))
        .collect::<HashMap<_, _>>();
    for entry in entries.iter_mut() {
        if let Some((rank, hits_per_day)) = ranking.get(entry.slug.as_str()) {
            entry.rank = *rank;
            entry.hits_per_day = *hits_per_day;
        }
    }
    entries.sort_by(|left, right| {
        let left_rank = if left.rank == 0 { u32::MAX } else { left.rank };
        let right_rank = if right.rank == 0 {
            u32::MAX
        } else {
            right.rank
        };
        left_rank
            .cmp(&right_rank)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
}

pub(crate) fn distribution_details(slug: &str) -> Result<DistributionDetails> {
    validate_slug(slug)?;
    let page_url = format!("{DISTROWATCH_BASE}table.php?distribution={slug}");
    let html = fetch_text(&page_url)?;
    parse_distribution(&html, slug, &page_url)
}

pub(crate) fn artwork(url: &str) -> Result<Vec<u8>> {
    let source = secure_url(url)?;
    let response = send(&metadata_client()?, source.as_str())?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_ARTWORK_BYTES)
    {
        return Err(Error::InvalidDownload(
            "catalog artwork exceeds the 16 MiB safety limit".into(),
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_ARTWORK_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| network_error(source.as_str(), error))?;
    if bytes.len() as u64 > MAX_ARTWORK_BYTES {
        return Err(Error::InvalidDownload(
            "catalog artwork exceeds the 16 MiB safety limit".into(),
        ));
    }
    Ok(bytes)
}

pub(crate) fn distribution_bundle(slug: &str) -> Result<DistributionBundle> {
    let details = distribution_details(slug)?;
    let mut releases = Vec::new();
    let mut warnings = Vec::new();
    for source in &details.download_pages {
        match iso_releases(source) {
            Ok(found) => releases.extend(found),
            Err(error) => warnings.push(error.to_string()),
        }
    }
    deduplicate_releases(&mut releases);
    Ok(DistributionBundle {
        details,
        releases,
        warnings,
    })
}

pub(crate) fn iso_releases(source_url: &str) -> Result<Vec<IsoRelease>> {
    let source = secure_url(source_url)?;
    if is_direct_iso(&source) {
        let mut releases = vec![IsoRelease {
            name: iso_name(&source).unwrap_or_else(|| "download.iso".into()),
            url: source.to_string(),
            size: None,
            published: None,
            checksum_algorithm: None,
            checksum: None,
            checksum_url: None,
        }];
        if let Some(directory) = iso_directory_url(&source)
            && let Ok(html) = fetch_text(directory.as_str())
            && let Ok((_, _, checksum_sources)) = parse_download_page(&html, &directory)
        {
            associate_checksum_sources(&mut releases, &checksum_sources);
        }
        return Ok(releases);
    }
    if let Some(rss_url) = sourceforge_rss_url(&source) {
        return parse_sourceforge_rss(&fetch_text(rss_url.as_str())?);
    }
    crawl_iso_releases(source)
}

pub(crate) fn download_iso(
    release: &IsoRelease,
    destination: &Path,
    control: &OperationControl,
    mut progress: impl FnMut(Progress),
) -> Result<()> {
    control.checkpoint()?;
    let source = secure_url(&release.url)?;
    let publisher_checksum = resolve_publisher_checksum(release)?;
    if !release.name.to_ascii_lowercase().ends_with(".iso") {
        return Err(Error::InvalidDownload(
            "the selected catalog entry is not an ISO image".into(),
        ));
    }
    if destination
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("iso"))
    {
        return Err(Error::InvalidDownload(
            "the download destination must end in .iso".into(),
        ));
    }
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
    ensure_workspace(parent, release.size)?;

    progress(Progress {
        phase: ProgressPhase::Preparing,
        completed: 0,
        total: release.size,
        message: format!("Stage 1/5 · Connecting securely for {}", release.name),
    });
    let staged = download::stage(
        &download_client()?,
        &source,
        destination,
        release.size,
        control,
        |transfer| {
            let completed = transfer.completed;
            let total = transfer.total;
            progress(Progress {
                phase: ProgressPhase::Downloading,
                completed,
                total,
                message: download::transfer_message("Stage 2/5 · Downloading", transfer),
            });
        },
    )?;
    let completed = staged.size();
    let total = Some(completed);
    progress(Progress {
        phase: ProgressPhase::Syncing,
        completed,
        total,
        message: "Stage 3/5 · Transfer complete · partial file synced".into(),
    });

    if let Some((algorithm, expected)) = publisher_checksum.as_ref() {
        progress(Progress {
            phase: ProgressPhase::Verifying,
            completed: 0,
            total,
            message: format!("Stage 4/5 · Verifying {algorithm} checksum"),
        });
        let actual = match checksum::compute_controlled(staged.path(), *algorithm, control) {
            Ok(actual) => actual,
            Err(error) => {
                if matches!(error, Error::OperationCancelled) {
                    staged.discard()?;
                }
                return Err(error);
            }
        };
        if !actual.hexadecimal.eq_ignore_ascii_case(expected) {
            staged.discard()?;
            return Err(Error::InvalidDownload(format!(
                "{} checksum mismatch for {}",
                algorithm, release.name
            )));
        }
    }
    if let Err(error) = control.checkpoint() {
        staged.discard()?;
        return Err(error);
    }
    staged.persist()?;
    progress(Progress {
        phase: ProgressPhase::Verifying,
        completed,
        total,
        message: match publisher_checksum {
            Some((algorithm, _)) => format!(
                "Stage 4/5 · Publisher {algorithm} verified · finalized at {}",
                destination.display()
            ),
            None => format!(
                "Stage 4/5 · HTTPS transfer length verified · publisher checksum unavailable · finalized at {}",
                destination.display()
            ),
        },
    });
    Ok(())
}

fn resolve_publisher_checksum(release: &IsoRelease) -> Result<Option<(ChecksumAlgorithm, String)>> {
    let embedded = match (release.checksum_algorithm, release.checksum.as_deref()) {
        (Some(algorithm), Some(value)) => Some((algorithm, validated_checksum(algorithm, value)?)),
        (Some(_), None) if release.checksum_url.is_some() => None,
        (None, None) => None,
        _ => {
            return Err(Error::InvalidDownload(format!(
                "incomplete checksum metadata for {}",
                release.name
            )));
        }
    };
    let Some(checksum_url) = release.checksum_url.as_deref() else {
        return Ok(embedded);
    };
    let checksum_url = secure_url(checksum_url)?;
    let document = match fetch_text(checksum_url.as_str()) {
        Ok(document) => document,
        Err(_error) if embedded.is_some() => return Ok(embedded),
        Err(error) => return Err(error),
    };
    let sidecar = parse_publisher_checksum(
        &document,
        &release.name,
        checksum_algorithm_from_url(&checksum_url),
    )
    .ok_or_else(|| {
        Error::InvalidDownload(format!(
            "publisher checksum file does not contain a supported digest for {}",
            release.name
        ))
    })?;
    Ok(match embedded {
        Some(current) if checksum_strength(current.0) > checksum_strength(sidecar.0) => {
            Some(current)
        }
        _ => Some(sidecar),
    })
}

fn validated_checksum(algorithm: ChecksumAlgorithm, value: &str) -> Result<String> {
    let value = value.trim();
    if checksum_algorithm_from_hex(value) != Some(algorithm) {
        return Err(Error::InvalidDownload(format!(
            "publisher {algorithm} checksum has an invalid format"
        )));
    }
    Ok(value.to_ascii_lowercase())
}

fn parse_popularity(html: &str, limit: usize) -> Result<Vec<DistributionSummary>> {
    let document = Html::parse_document(html);
    let period_selector = selector("th.Invert")?;
    let row_selector = selector("tr")?;
    let rank_selector = selector("th.phr1")?;
    let distro_selector = selector("td.phr2 a")?;
    let hits_selector = selector("td.phr3")?;
    let period = document
        .select(&period_selector)
        .find(|header| normalized_text(*header) == "Last 6 months")
        .ok_or_else(|| Error::InvalidCatalog("six-month popularity table is missing".into()))?;
    let table = period
        .ancestors()
        .filter_map(ElementRef::wrap)
        .find(|element| element.value().name() == "table")
        .ok_or_else(|| Error::InvalidCatalog("six-month popularity table is missing".into()))?;
    let mut distributions = Vec::new();
    for row in table.select(&row_selector) {
        let Some(rank) = row
            .select(&rank_selector)
            .next()
            .and_then(|value| digits(&normalized_text(value)))
        else {
            continue;
        };
        let Some(link) = row.select(&distro_selector).next() else {
            continue;
        };
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let Some(slug) = distro_slug(href) else {
            continue;
        };
        let hits_per_day = row
            .select(&hits_selector)
            .next()
            .and_then(|value| digits(&normalized_text(value)))
            .unwrap_or_default();
        let based_on = link
            .value()
            .attr("title")
            .and_then(|title| title.strip_prefix("Based on: "))
            .map(str::to_owned);
        distributions.push(DistributionSummary {
            rank,
            name: normalized_text(link),
            page_url: format!("{DISTROWATCH_BASE}table.php?distribution={slug}"),
            logo_url: format!("{DISTROWATCH_BASE}images/icon-large/{slug}.png"),
            slug,
            hits_per_day,
            based_on,
        });
        if distributions.len() >= limit {
            break;
        }
    }
    if distributions.is_empty() {
        return Err(Error::InvalidCatalog(
            "DistroWatch returned no ranked distributions".into(),
        ));
    }
    Ok(distributions)
}

fn parse_distribution_directory(html: &str) -> Result<Vec<DistributionSummary>> {
    let document = Html::parse_document(html);
    let options = selector("select[name=distribution] option[value]")?;
    let mut entries = document
        .select(&options)
        .filter_map(|option| {
            let slug = option.value().attr("value")?.trim();
            let name = normalized_text(option);
            if slug.is_empty() || name.is_empty() || validate_slug(slug).is_err() {
                return None;
            }
            Some(distribution_summary(0, name, slug, 0, None))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    entries.dedup_by(|left, right| left.slug == right.slug);
    if entries.is_empty() {
        return Err(Error::InvalidCatalog(
            "DistroWatch distribution directory is missing".into(),
        ));
    }
    Ok(entries)
}

fn parse_base_search(html: &str, base: &str) -> Result<Vec<DistributionSummary>> {
    let document = Html::parse_document(html);
    let links = selector("b > a[href]")?;
    let mut seen = HashSet::new();
    let mut entries = document
        .select(&links)
        .filter_map(|link| {
            let slug = link.value().attr("href")?.trim();
            let name = normalized_text(link);
            if slug.is_empty()
                || name.is_empty()
                || validate_slug(slug).is_err()
                || !seen.insert(slug.to_owned())
            {
                return None;
            }
            Some(distribution_summary(
                0,
                name,
                slug,
                0,
                Some(base.to_owned()),
            ))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    if entries.is_empty() {
        return Err(Error::InvalidCatalog(format!(
            "DistroWatch returned no active {base}-based distributions"
        )));
    }
    Ok(entries)
}

fn distribution_summary(
    rank: u32,
    name: String,
    slug: &str,
    hits_per_day: u32,
    based_on: Option<String>,
) -> DistributionSummary {
    DistributionSummary {
        rank,
        name,
        slug: slug.to_owned(),
        hits_per_day,
        based_on,
        page_url: format!("{DISTROWATCH_BASE}table.php?distribution={slug}"),
        logo_url: format!("{DISTROWATCH_BASE}images/icon-large/{slug}.png"),
    }
}

fn parse_distribution(html: &str, slug: &str, page_url: &str) -> Result<DistributionDetails> {
    let document = Html::parse_document(html);
    let heading_selector = selector("td.TablesTitle h1, h1")?;
    let row_selector = selector("tr")?;
    let header_selector = selector("th")?;
    let cell_selector = selector("td")?;
    let link_selector = selector("a")?;
    let profile_selector = selector("td.TablesTitle")?;
    let logo_selector = selector("img.logo")?;
    let screenshot_selector = selector("a[href*='images/slinks/']")?;
    let gallery_selector = selector("a[href*='gallery.php?distribution=']")?;
    let item_selector = selector("li")?;
    let update_selector = selector("h2")?;
    let name = document
        .select(&heading_selector)
        .map(normalized_text)
        .find(|name| !name.is_empty())
        .ok_or_else(|| Error::InvalidCatalog("distribution name is missing".into()))?;
    let mut home_page = None;
    let mut release_date = None;
    let mut image_size = None;
    let mut free_downloads = Vec::new();
    let mut mirrors = Vec::new();
    let mut documentation_pages = Vec::new();
    let mut screenshot_pages = Vec::new();
    for row in document.select(&row_selector) {
        let Some(header) = row.select(&header_selector).next() else {
            continue;
        };
        let label = normalized_text(header);
        let cells = row.select(&cell_selector).collect::<Vec<_>>();
        match label.as_str() {
            "Home Page" => home_page = first_https_link(&row, &link_selector),
            "Download Mirrors" => collect_https_links(&row, &link_selector, &mut mirrors),
            "Free Download" => collect_https_links(&row, &link_selector, &mut free_downloads),
            "Documentation" => {
                collect_page_links(&row, &link_selector, page_url, &mut documentation_pages)
            }
            "Screenshots" => {
                collect_page_links(&row, &link_selector, page_url, &mut screenshot_pages)
            }
            "Release Date" => release_date = cells.first().map(|cell| normalized_text(*cell)),
            "Image Size (MB)" => image_size = cells.first().map(|cell| normalized_text(*cell)),
            _ => {}
        }
    }
    free_downloads.extend(mirrors);
    deduplicate(&mut free_downloads);
    deduplicate(&mut documentation_pages);
    deduplicate(&mut screenshot_pages);
    if free_downloads.is_empty() {
        return Err(Error::InvalidCatalog(format!(
            "DistroWatch lists no ISO source for {name}"
        )));
    }
    let profile = document.select(&profile_selector).next();
    let profile_item = |label: &str| {
        profile.and_then(|profile| {
            profile
                .select(&item_selector)
                .map(normalized_text)
                .find_map(|text| text.strip_prefix(label).map(str::trim).map(str::to_owned))
        })
    };
    let profile_list = |label: &str| {
        profile_item(label)
            .map(|value| split_profile_list(&value))
            .unwrap_or_default()
    };
    let profile_text = profile.map(normalized_text).unwrap_or_default();
    let (visitor_rating, visitor_review_count) = parse_visitor_rating(&profile_text);
    let logo_url = profile
        .and_then(|profile| profile.select(&logo_selector).next())
        .and_then(|image| image.value().attr("src"))
        .and_then(|value| absolute_https_url(page_url, value));
    let screenshot_url = profile
        .and_then(|profile| profile.select(&screenshot_selector).next())
        .and_then(|link| link.value().attr("href"))
        .and_then(|value| absolute_https_url(page_url, value));
    let gallery_url = profile
        .and_then(|profile| profile.select(&gallery_selector).next())
        .and_then(|link| link.value().attr("href"))
        .and_then(|value| absolute_https_url(page_url, value));
    let last_update = profile
        .and_then(|profile| profile.select(&update_selector).next())
        .map(normalized_text)
        .and_then(|value| {
            value
                .strip_prefix("Last Update:")
                .map(str::trim)
                .map(str::to_owned)
        });

    Ok(DistributionDetails {
        name,
        slug: slug.into(),
        page_url: page_url.into(),
        home_page,
        release_date: release_date.filter(|value| !value.is_empty()),
        image_size: image_size.filter(|value| !value.is_empty()),
        download_pages: free_downloads,
        logo_url,
        screenshot_url,
        gallery_url,
        description: profile.and_then(profile_description),
        os_type: profile_item("OS Type:"),
        based_on: profile_item("Based on:"),
        origin: profile_item("Origin:"),
        architectures: profile_list("Architecture:"),
        desktops: profile_list("Desktop:"),
        categories: profile_list("Category:"),
        status: profile_item("Status:"),
        last_update,
        visitor_rating,
        visitor_review_count,
        documentation_pages,
        screenshot_pages,
    })
}

fn parse_sourceforge_rss(xml: &str) -> Result<Vec<IsoRelease>> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| Error::InvalidCatalog(format!("invalid SourceForge RSS: {error}")))?;
    let items = document
        .descendants()
        .filter(|node| node.has_tag_name("item"))
        .filter_map(|item| {
            let title = child_text(item, "title")?;
            let link = child_text(item, "link")?;
            Some((item, title.to_owned(), link.to_owned()))
        })
        .collect::<Vec<_>>();
    let mut releases = Vec::new();
    for (item, title, link) in &items {
        if !title.to_ascii_lowercase().ends_with(".iso") {
            continue;
        }
        let content = item.descendants().find(|node| {
            node.tag_name().name() == "content"
                && node.attribute("url").is_some_and(|url| url == link)
        });
        let size = content
            .and_then(|node| node.attribute("filesize"))
            .and_then(|value| value.parse().ok());
        let md5 = content.and_then(|node| {
            node.children()
                .find(|child| child.tag_name().name() == "hash")
                .filter(|hash| hash.attribute("algo") == Some("md5"))
                .and_then(|hash| hash.text())
                .map(str::to_owned)
        });
        let checksum_title = format!("{title}.sha256");
        let checksum_url = items
            .iter()
            .find(|(_, candidate, _)| candidate == &checksum_title)
            .map(|(_, _, url)| url.clone());
        releases.push(IsoRelease {
            name: title.rsplit('/').next().unwrap_or(title).to_owned(),
            url: link.clone(),
            size,
            published: child_text(*item, "pubDate").map(str::to_owned),
            checksum_algorithm: md5.as_ref().map(|_| ChecksumAlgorithm::Md5),
            checksum: md5,
            checksum_url,
        });
    }
    deduplicate_releases(&mut releases);
    if releases.is_empty() {
        return Err(Error::InvalidCatalog(
            "the ISO source returned no direct ISO files".into(),
        ));
    }
    Ok(releases)
}

#[cfg(test)]
fn parse_iso_links(html: &str, base: &Url) -> Result<Vec<IsoRelease>> {
    let (releases, _, _) = parse_download_page(html, base)?;
    if releases.is_empty() {
        return Err(Error::InvalidCatalog(
            "the ISO source returned no direct ISO links".into(),
        ));
    }
    Ok(releases)
}

fn crawl_iso_releases(source: Url) -> Result<Vec<IsoRelease>> {
    let mut queue = VecDeque::from([(source, 0_usize)]);
    let mut visited = HashSet::new();
    let mut releases = Vec::new();
    let mut checksum_sources = Vec::new();
    let mut checksum_directories = HashSet::new();
    let mut last_error = None;
    while let Some((page, depth)) = queue.pop_front() {
        if visited.len() >= MAX_CRAWL_PAGES || !visited.insert(page.to_string()) {
            continue;
        }
        let html = match fetch_text(page.as_str()) {
            Ok(html) => html,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };
        let (found, links, checksums) = parse_download_page(&html, &page)?;
        for directory in found
            .iter()
            .filter_map(|release| Url::parse(&release.url).ok())
            .filter_map(|url| iso_directory_url(&url))
        {
            if checksum_directories.len() >= MAX_CHECKSUM_DIRECTORY_PAGES {
                break;
            }
            if checksum_directories.insert(directory.to_string())
                && let Ok(directory_html) = fetch_text(directory.as_str())
                && let Ok((_, _, adjacent)) = parse_download_page(&directory_html, &directory)
            {
                checksum_sources.extend(adjacent);
            }
        }
        releases.extend(found);
        checksum_sources.extend(checksums);
        if depth < MAX_CRAWL_DEPTH {
            for link in links {
                if visited.len() + queue.len() >= MAX_CRAWL_PAGES {
                    break;
                }
                if !visited.contains(link.as_str()) {
                    queue.push_back((link, depth + 1));
                }
            }
        }
    }
    deduplicate_releases(&mut releases);
    deduplicate_urls(&mut checksum_sources);
    associate_checksum_sources(&mut releases, &checksum_sources);
    if releases.is_empty() {
        if let Some(error) = last_error {
            return Err(error);
        }
        return Err(Error::InvalidCatalog(
            "the ISO source returned no direct ISO links within the bounded search".into(),
        ));
    }
    Ok(releases)
}

fn parse_download_page(html: &str, base: &Url) -> Result<(Vec<IsoRelease>, Vec<Url>, Vec<Url>)> {
    let document = Html::parse_document(html);
    let links = selector("a[href]")?;
    let elements = selector("*")?;
    let mut releases = Vec::new();
    let mut crawl_links = Vec::new();
    let mut checksum_links = Vec::new();
    for link in document.select(&links) {
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let Ok(url) = base.join(href) else {
            continue;
        };
        if url.scheme() != "https" {
            continue;
        }
        if is_direct_iso(&url) {
            push_iso_release(&mut releases, url);
        } else if is_checksum_url(&url) {
            checksum_links.push(url);
        } else if same_origin(base, &url) && is_crawl_candidate(&url, &normalized_text(link)) {
            crawl_links.push(url);
        }
    }
    for element in document.select(&elements) {
        for (_, value) in element.value().attrs() {
            for url in embedded_https_urls(value) {
                if is_direct_iso(&url) {
                    push_iso_release(&mut releases, url);
                }
            }
        }
    }
    deduplicate_releases(&mut releases);
    let mut seen = HashSet::new();
    crawl_links.retain(|url| seen.insert(url.to_string()));
    deduplicate_urls(&mut checksum_links);
    associate_checksum_sources(&mut releases, &checksum_links);
    Ok((releases, crawl_links, checksum_links))
}

fn associate_checksum_sources(releases: &mut [IsoRelease], sources: &[Url]) {
    for release in releases {
        let release_name = release.name.to_ascii_lowercase();
        let source = sources
            .iter()
            .filter_map(|source| {
                let file_name = source.path_segments()?.next_back()?.to_ascii_lowercase();
                let direct =
                    checksum_target_name(&file_name).is_some_and(|target| target == release_name);
                let generic = is_generic_checksum_name(&file_name);
                (direct || generic).then_some((source, direct))
            })
            .max_by_key(|(source, direct)| {
                (
                    *direct,
                    checksum_algorithm_from_url(source)
                        .map(checksum_strength)
                        .unwrap_or_default(),
                )
            });
        if let Some((source, _)) = source {
            release.checksum_url = Some(source.to_string());
            if release.checksum.is_none() {
                release.checksum_algorithm = checksum_algorithm_from_url(source);
            }
        }
    }
}

fn is_checksum_url(url: &Url) -> bool {
    let Some(name) = url.path_segments().and_then(Iterator::last) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    checksum_target_name(&name).is_some() || is_generic_checksum_name(&name)
}

fn checksum_target_name(name: &str) -> Option<String> {
    for suffix in [
        ".sha512",
        ".sha512sum",
        ".sha512.txt",
        ".sha256",
        ".sha256sum",
        ".sha256.txt",
        ".sha1",
        ".sha1sum",
        ".sha1.txt",
        ".md5",
        ".md5sum",
        ".md5.txt",
    ] {
        if let Some(target) = name.strip_suffix(suffix) {
            return Some(target.to_owned());
        }
    }
    None
}

fn is_generic_checksum_name(name: &str) -> bool {
    matches!(
        name,
        "checksum"
            | "checksums"
            | "checksum.txt"
            | "checksums.txt"
            | "md5sums"
            | "md5sums.txt"
            | "sha1sums"
            | "sha1sums.txt"
            | "sha256sums"
            | "sha256sums.txt"
            | "sha512sums"
            | "sha512sums.txt"
    )
}

fn checksum_algorithm_from_url(url: &Url) -> Option<ChecksumAlgorithm> {
    let name = url.path_segments()?.next_back()?.to_ascii_lowercase();
    if name.contains("sha512") {
        Some(ChecksumAlgorithm::Sha512)
    } else if name.contains("sha256") {
        Some(ChecksumAlgorithm::Sha256)
    } else if name.contains("sha1") {
        Some(ChecksumAlgorithm::Sha1)
    } else if name.contains("md5") {
        Some(ChecksumAlgorithm::Md5)
    } else {
        None
    }
}

fn parse_publisher_checksum(
    document: &str,
    release_name: &str,
    algorithm_hint: Option<ChecksumAlgorithm>,
) -> Option<(ChecksumAlgorithm, String)> {
    let release_name = release_name.trim_start_matches("./");
    let mut candidates = Vec::new();
    for line in document
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some((left, value)) = line.split_once(" = ")
            && let Some((_, file_name)) = left.split_once('(')
            && file_name.trim_end_matches(')').trim_start_matches("./") == release_name
        {
            push_checksum_candidate(&mut candidates, value, algorithm_hint);
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(value) = fields.next() else { continue };
        match fields.next() {
            Some(file_name)
                if file_name.trim_start_matches('*').trim_start_matches("./") == release_name =>
            {
                push_checksum_candidate(&mut candidates, value, algorithm_hint);
            }
            None if algorithm_hint.is_some()
                && document
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count()
                    == 1 =>
            {
                push_checksum_candidate(&mut candidates, value, algorithm_hint);
            }
            _ => {}
        }
    }
    candidates
        .into_iter()
        .max_by_key(|(algorithm, _)| checksum_strength(*algorithm))
}

fn push_checksum_candidate(
    candidates: &mut Vec<(ChecksumAlgorithm, String)>,
    value: &str,
    algorithm_hint: Option<ChecksumAlgorithm>,
) {
    let value = value.trim();
    let Some(algorithm) = checksum_algorithm_from_hex(value) else {
        return;
    };
    if algorithm_hint.is_none_or(|hint| hint == algorithm) {
        candidates.push((algorithm, value.to_ascii_lowercase()));
    }
}

fn checksum_algorithm_from_hex(value: &str) -> Option<ChecksumAlgorithm> {
    if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    match value.len() {
        32 => Some(ChecksumAlgorithm::Md5),
        40 => Some(ChecksumAlgorithm::Sha1),
        64 => Some(ChecksumAlgorithm::Sha256),
        128 => Some(ChecksumAlgorithm::Sha512),
        _ => None,
    }
}

fn checksum_strength(algorithm: ChecksumAlgorithm) -> u8 {
    match algorithm {
        ChecksumAlgorithm::Md5 => 1,
        ChecksumAlgorithm::Sha1 => 2,
        ChecksumAlgorithm::Sha256 => 3,
        ChecksumAlgorithm::Sha512 => 4,
    }
}

fn deduplicate_urls(values: &mut Vec<Url>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.to_string()));
}

fn push_iso_release(releases: &mut Vec<IsoRelease>, url: Url) {
    let Some(name) = iso_name(&url) else {
        return;
    };
    releases.push(IsoRelease {
        name,
        url: url.to_string(),
        size: None,
        published: None,
        checksum_algorithm: None,
        checksum: None,
        checksum_url: None,
    });
}

fn embedded_https_urls(value: &str) -> Vec<Url> {
    value
        .match_indices("https://")
        .filter_map(|(start, _)| {
            let candidate = &value[start..];
            let end = candidate
                .find(|character: char| {
                    character.is_whitespace()
                        || matches!(character, '\'' | '"' | '<' | '>' | ')' | ';')
                })
                .unwrap_or(candidate.len());
            Url::parse(&candidate[..end]).ok()
        })
        .collect()
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn is_crawl_candidate(url: &Url, label: &str) -> bool {
    let candidate = format!("{} {label}", url.path()).to_ascii_lowercase();
    [
        "download",
        "edition",
        "current",
        "amd64",
        "x86_64",
        "x64",
        "iso-hybrid",
        "release",
    ]
    .iter()
    .any(|token| candidate.contains(token))
        && !candidate.contains("torrent")
        && !candidate.contains("archive")
}

fn deduplicate_releases(releases: &mut Vec<IsoRelease>) {
    let mut merged: Vec<IsoRelease> = Vec::with_capacity(releases.len());
    let mut positions = HashMap::new();
    for release in releases.drain(..) {
        let key = release.name.to_ascii_lowercase();
        if let Some(position) = positions.get(&key).copied() {
            merge_release_metadata(&mut merged[position], release);
        } else {
            positions.insert(key, merged.len());
            merged.push(release);
        }
    }
    *releases = merged;
}

fn merge_release_metadata(existing: &mut IsoRelease, candidate: IsoRelease) {
    if existing.size.is_none() {
        existing.size = candidate.size;
    }
    if existing.published.is_none() {
        existing.published = candidate.published;
    }
    let existing_strength = existing
        .checksum_algorithm
        .filter(|_| existing.checksum.is_some() || existing.checksum_url.is_some())
        .map(checksum_strength)
        .unwrap_or_default();
    let candidate_strength = candidate
        .checksum_algorithm
        .filter(|_| candidate.checksum.is_some() || candidate.checksum_url.is_some())
        .map(checksum_strength)
        .unwrap_or_default();
    if candidate_strength > existing_strength
        || (candidate_strength == existing_strength
            && existing.checksum_url.is_none()
            && candidate.checksum_url.is_some())
    {
        existing.checksum_algorithm = candidate.checksum_algorithm;
        existing.checksum = candidate.checksum;
        existing.checksum_url = candidate.checksum_url;
    }
}

fn metadata_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(45))
        .redirect(Policy::limited(10))
        .build()
        .map_err(|error| network_error("client setup", error))
}

fn download_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .redirect(Policy::limited(10))
        .build()
        .map_err(|error| network_error("client setup", error))
}

fn fetch_text(url: &str) -> Result<String> {
    let response = send(&metadata_client()?, url)?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_CATALOG_BYTES)
    {
        return Err(Error::InvalidCatalog(
            "catalog response is too large".into(),
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_CATALOG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| network_error(url, error))?;
    if bytes.len() as u64 > MAX_CATALOG_BYTES {
        return Err(Error::InvalidCatalog(
            "catalog response is too large".into(),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|error| Error::InvalidCatalog(format!("catalog is not UTF-8: {error}")))
}

fn send(client: &Client, url: &str) -> Result<Response> {
    let response = client
        .get(url)
        .send()
        .and_then(Response::error_for_status)
        .map_err(|error| network_error(url, error))?;
    if response.url().scheme() != "https" {
        return Err(Error::InvalidDownload(
            "catalog request redirected to an insecure URL".into(),
        ));
    }
    Ok(response)
}

fn network_error(url: &str, error: impl std::fmt::Display) -> Error {
    Error::Network {
        url: url.into(),
        message: error.to_string(),
    }
}

fn secure_url(value: &str) -> Result<Url> {
    let url = Url::parse(value)
        .map_err(|error| Error::InvalidDownload(format!("invalid URL: {error}")))?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err(Error::InvalidDownload(
            "catalog downloads require an absolute HTTPS URL".into(),
        ));
    }
    Ok(url)
}

fn sourceforge_rss_url(source: &Url) -> Option<Url> {
    if source.host_str()? != "sourceforge.net" {
        return None;
    }
    let segments = source.path_segments()?.collect::<Vec<_>>();
    if segments.len() < 4 || segments[0] != "projects" || segments[2] != "files" {
        return None;
    }
    let project = segments[1];
    let path = segments[3..]
        .iter()
        .filter(|segment| !segment.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    let mut rss = Url::parse(&format!("https://sourceforge.net/projects/{project}/rss")).ok()?;
    rss.query_pairs_mut()
        .append_pair("path", &format!("/{path}"));
    Some(rss)
}

fn validate_slug(slug: &str) -> Result<()> {
    if slug.is_empty()
        || slug.len() > 64
        || !slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(Error::InvalidCatalog(format!(
            "invalid DistroWatch distribution slug `{slug}`"
        )));
    }
    Ok(())
}

fn selector(value: &str) -> Result<Selector> {
    Selector::parse(value)
        .map_err(|error| Error::InvalidCatalog(format!("invalid HTML selector: {error}")))
}

fn normalized_text(element: ElementRef<'_>) -> String {
    element
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

fn digits(value: &str) -> Option<u32> {
    value
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

fn distro_slug(href: &str) -> Option<String> {
    let candidate = href
        .strip_prefix("table.php?distribution=")
        .unwrap_or(href)
        .trim_matches('/');
    validate_slug(candidate).ok()?;
    Some(candidate.into())
}

fn first_https_link(row: &ElementRef<'_>, selector: &Selector) -> Option<String> {
    row.select(selector)
        .filter_map(|link| link.value().attr("href"))
        .find(|href| href.starts_with("https://"))
        .map(str::to_owned)
}

fn collect_https_links(row: &ElementRef<'_>, selector: &Selector, output: &mut Vec<String>) {
    output.extend(
        row.select(selector)
            .filter_map(|link| link.value().attr("href"))
            .filter(|href| href.starts_with("https://"))
            .map(str::to_owned),
    );
}

fn collect_page_links(
    row: &ElementRef<'_>,
    selector: &Selector,
    base: &str,
    output: &mut Vec<String>,
) {
    output.extend(
        row.select(selector)
            .filter_map(|link| link.value().attr("href"))
            .filter_map(|href| absolute_https_url(base, href)),
    );
}

fn absolute_https_url(base: &str, value: &str) -> Option<String> {
    let base = Url::parse(base).ok()?;
    let url = base.join(value).ok()?;
    (url.scheme() == "https").then(|| url.to_string())
}

fn split_profile_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn profile_description(profile: ElementRef<'_>) -> Option<String> {
    let html = profile.inner_html();
    let after_profile = html.split_once("</ul>")?.1;
    let description_html = after_profile.split("<br").next()?.trim();
    let fragment = Html::parse_fragment(description_html);
    let description = normalized_text(fragment.root_element());
    (!description.is_empty()).then_some(description)
}

fn parse_visitor_rating(profile: &str) -> (Option<String>, Option<u32>) {
    let Some(rating_section) = profile.split("Average visitor rating").nth(1) else {
        return (None, None);
    };
    let rating = rating_section
        .split("/10")
        .next()
        .and_then(|value| value.rsplit(':').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let reviews = rating_section
        .split("from")
        .nth(1)
        .and_then(|value| value.split_whitespace().next())
        .and_then(digits);
    (rating, reviews)
}

fn child_text<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|child| child.tag_name().name() == name)
        .and_then(|child| child.text())
}

fn is_direct_iso(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    path.ends_with(".iso") || path.ends_with(".iso/download")
}

fn iso_directory_url(url: &Url) -> Option<Url> {
    is_direct_iso(url).then_some(())?;
    let mut directory = url.clone();
    let sourceforge_download = directory
        .path()
        .to_ascii_lowercase()
        .ends_with(".iso/download");
    {
        let mut segments = directory.path_segments_mut().ok()?;
        if sourceforge_download {
            segments.pop();
        }
        segments.pop();
        segments.push("");
    }
    directory.set_query(None);
    directory.set_fragment(None);
    Some(directory)
}

fn iso_name(url: &Url) -> Option<String> {
    let segments = url.path_segments()?.collect::<Vec<_>>();
    let candidate = if segments.last().copied() == Some("download") {
        segments.iter().rev().nth(1).copied()
    } else {
        segments.last().copied()
    }?;
    candidate
        .to_ascii_lowercase()
        .ends_with(".iso")
        .then(|| candidate.to_owned())
}

fn deduplicate(values: &mut Vec<String>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_six_month_popularity_rows() {
        let html = r#"
            <table><tr><th class="Invert">Last 12 months</th></tr></table>
            <table><tr><th class="Invert" colspan="3">Last 6 months</th></tr>
              <tr><th class="phr1">1</th><td class="phr2"><a title="Based on: Arch" href="cachyos">CachyOS</a></td><td class="phr3">3,012</td></tr>
              <tr><th class="phr1">2</th><td class="phr2"><a title="Based on: Debian" href="mx">MX Linux</a></td><td class="phr3">2,100</td></tr>
            </table>"#;
        let entries = parse_popularity(html, 1).expect("popularity");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].slug, "cachyos");
        assert_eq!(entries[0].hits_per_day, 3012);
        assert_eq!(entries[0].based_on.as_deref(), Some("Arch"));
    }

    #[test]
    fn parses_full_distribution_directory() {
        let html = r#"<select name="distribution">
          <option value="">Select Distribution</option>
          <option value="arch">Arch</option>
          <option value="omarchy">Omarchy</option>
          <option value="bad slug">Invalid</option>
        </select>"#;
        let entries = parse_distribution_directory(html).expect("directory");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].slug, "arch");
        assert_eq!(entries[1].slug, "omarchy");
        assert_eq!(entries[1].rank, 0);
    }

    #[test]
    fn base_results_follow_popularity_then_name() {
        let mut entries = vec![
            distribution_summary(0, "Unranked B".into(), "unranked-b", 0, Some("Arch".into())),
            distribution_summary(
                0,
                "Popular Two".into(),
                "popular-two",
                0,
                Some("Arch".into()),
            ),
            distribution_summary(
                0,
                "Popular One".into(),
                "popular-one",
                0,
                Some("Arch".into()),
            ),
            distribution_summary(0, "Unranked A".into(), "unranked-a", 0, Some("Arch".into())),
        ];
        let popularity = vec![
            distribution_summary(1, "Popular One".into(), "popular-one", 3000, None),
            distribution_summary(2, "Popular Two".into(), "popular-two", 2000, None),
        ];

        rank_base_results(&mut entries, &popularity);

        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.slug.as_str(), entry.rank, entry.hits_per_day))
                .collect::<Vec<_>>(),
            vec![
                ("popular-one", 1, 3000),
                ("popular-two", 2, 2000),
                ("unranked-a", 0, 0),
                ("unranked-b", 0, 0),
            ]
        );
    }

    #[test]
    fn parses_based_on_search_results() {
        let html = r#"<a id="simpleresults"></a>
          <b>1. <a href="cachyos">CachyOS</a> (1)</b><br>
          <b>2. <a href="omarchy">Omarchy</a> (20)</b>"#;
        let entries = parse_base_search(html, "Arch").expect("base search");
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .all(|entry| entry.based_on.as_deref() == Some("Arch"))
        );
    }

    #[test]
    fn parses_distrowatch_iso_sources() {
        let html = r#"
            <table><tr><td class="TablesTitle">
              <img class="logo" src="images/icon-large/cachyos.png">
              <h1>CachyOS</h1><h2>Last Update: 2026-08-09 18:02 UTC</h2>
              <a href="images/slinks/cachyos.png"><img src="images/slinks/cachyos-small.png"></a>
              <a href="gallery.php?distribution=cachyos">gallery</a>
              <ul><li><b>OS Type:</b> Linux</li><li><b>Based on:</b> Arch</li>
                <li><b>Origin:</b> Germany</li><li><b>Architecture:</b> x86_64, x86-64-v3</li>
                <li><b>Desktop:</b> KDE Plasma, GNOME</li><li><b>Category:</b> Desktop, Live Medium</li>
                <li><b>Status:</b> Active</li></ul>
              CachyOS is a fast Arch-based distribution.<br><br>
              <b>Average visitor rating</b>: <b>8.1</b>/10 from <b>506</b> review(s).
            </td></tr></table><table>
              <tr><th>Home Page</th><td><a href="https://cachyos.org/">home</a></td></tr>
              <tr><th>Documentation</th><td><a href="https://wiki.cachyos.org/">docs</a></td></tr>
              <tr><th>Screenshots</th><td><a href="gallery.php?distribution=cachyos">gallery</a></td></tr>
              <tr><th>Download Mirrors</th><td><a href="https://cachyos.org/download">mirror</a></td></tr>
              <tr><th>Release Date</th><td>2026-08-09</td></tr>
              <tr><th>Image Size (MB)</th><td>3000-3100</td></tr>
              <tr><th>Free Download</th><td><a href="https://sourceforge.net/projects/cachyos-arch/files/gui-installer/">ISO</a></td></tr>
            </table>"#;
        let details = parse_distribution(
            html,
            "cachyos",
            "https://distrowatch.com/table.php?distribution=cachyos",
        )
        .expect("details");
        assert_eq!(details.release_date.as_deref(), Some("2026-08-09"));
        assert_eq!(details.download_pages.len(), 2);
        assert!(details.download_pages[0].contains("sourceforge.net"));
        assert_eq!(details.os_type.as_deref(), Some("Linux"));
        assert_eq!(details.architectures, ["x86_64", "x86-64-v3"]);
        assert_eq!(details.desktops, ["KDE Plasma", "GNOME"]);
        assert_eq!(details.visitor_rating.as_deref(), Some("8.1"));
        assert_eq!(details.visitor_review_count, Some(506));
        assert_eq!(
            details.screenshot_url.as_deref(),
            Some("https://distrowatch.com/images/slinks/cachyos.png")
        );
        assert_eq!(details.documentation_pages.len(), 1);
        assert_eq!(details.screenshot_pages.len(), 1);
    }

    #[test]
    fn parses_sourceforge_iso_size_and_hash() {
        let xml = r#"<rss xmlns:media="http://video.search.yahoo.com/mrss/"><channel>
          <item><title>/desktop/test.iso.sha256</title><link>https://example.com/test.iso.sha256</link></item>
          <item><title>/desktop/test.iso</title><link>https://example.com/test.iso/download</link><pubDate>today</pubDate>
            <media:content url="https://example.com/test.iso/download" filesize="42"><media:hash algo="md5">abcd</media:hash></media:content>
          </item>
        </channel></rss>"#;
        let releases = parse_sourceforge_rss(xml).expect("releases");
        assert_eq!(releases[0].name, "test.iso");
        assert_eq!(releases[0].size, Some(42));
        assert_eq!(releases[0].checksum.as_deref(), Some("abcd"));
        assert_eq!(
            releases[0].checksum_url.as_deref(),
            Some("https://example.com/test.iso.sha256")
        );
    }

    #[test]
    fn sourceforge_folder_becomes_rss_feed() {
        let source =
            Url::parse("https://sourceforge.net/projects/cachyos-arch/files/gui-installer/")
                .expect("URL");
        assert_eq!(
            sourceforge_rss_url(&source).expect("RSS").as_str(),
            "https://sourceforge.net/projects/cachyos-arch/rss?path=%2Fgui-installer"
        );
    }

    #[test]
    fn iso_parent_directory_is_bounded_to_the_exact_https_path() {
        let direct = Url::parse(
            "https://cdimage.example.org/current/amd64/iso-cd/debian.iso?mirror=1#download",
        )
        .expect("URL");
        assert_eq!(
            iso_directory_url(&direct).expect("directory").as_str(),
            "https://cdimage.example.org/current/amd64/iso-cd/"
        );
        let sourceforge =
            Url::parse("https://example.org/releases/image.iso/download").expect("URL");
        assert_eq!(
            iso_directory_url(&sourceforge).expect("directory").as_str(),
            "https://example.org/releases/"
        );
        assert!(
            iso_directory_url(&Url::parse("https://example.org/releases/").expect("URL")).is_none()
        );
    }

    #[test]
    #[ignore = "live HTTPS validation for docs/validation.md"]
    fn live_direct_debian_iso_discovers_publisher_manifest() {
        let releases = iso_releases(
            "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/debian-13.6.0-amd64-netinst.iso",
        )
        .expect("direct Debian release");
        assert_eq!(releases.len(), 1);
        assert_eq!(
            releases[0].checksum_algorithm,
            Some(ChecksumAlgorithm::Sha512)
        );
        assert!(
            releases[0]
                .checksum_url
                .as_deref()
                .is_some_and(|url| url.ends_with("/SHA512SUMS"))
        );
    }

    #[test]
    #[ignore = "live HTTPS validation for docs/validation.md"]
    fn live_distrowatch_debian_bundle_preserves_publisher_manifest() {
        let bundle = distribution_bundle("debian").expect("Debian bundle");
        assert!(
            bundle
                .releases
                .iter()
                .all(|release| release.checksum_url.is_some()),
            "bundle releases: {:?}",
            bundle.releases
        );
    }

    #[test]
    fn rejects_insecure_or_relative_download_urls() {
        for value in ["http://example.com/image.iso", "/image.iso", "image.iso"] {
            assert!(secure_url(value).is_err(), "accepted {value}");
        }
        assert!(secure_url("https://example.com/image.iso").is_ok());
    }

    #[test]
    fn rejects_unsafe_distribution_slugs() {
        for slug in ["", "CachyOS", "../cachyos", "cachyos&x=1"] {
            assert!(validate_slug(slug).is_err(), "accepted {slug}");
        }
        assert!(validate_slug("pop-os").is_ok());
    }

    #[test]
    fn resolves_only_https_iso_links() {
        let html = r#"
          <a href="image.iso">current</a>
          <a href="https://cdn.example.com/other.iso">mirror</a>
          <a href="SHA256SUMS">checksums</a>
          <a href="http://cdn.example.com/insecure.iso">insecure</a>
          <a href="notes.txt">notes</a>
        "#;
        let releases = parse_iso_links(
            html,
            &Url::parse("https://downloads.example.com/releases/").expect("base URL"),
        )
        .expect("ISO links");
        assert_eq!(releases.len(), 2);
        assert!(
            releases
                .iter()
                .all(|release| release.url.starts_with("https://"))
        );
        assert!(releases.iter().all(|release| {
            release.checksum_url.as_deref()
                == Some("https://downloads.example.com/releases/SHA256SUMS")
        }));
    }

    #[test]
    fn parses_gnu_bsd_and_direct_publisher_checksums() {
        let sha256 = "a".repeat(64);
        let sha512 = "b".repeat(128);
        let document =
            format!("{sha256} *other.iso\nSHA512 (image.iso) = {sha512}\n{sha256}  image.iso\n");
        assert_eq!(
            parse_publisher_checksum(&document, "image.iso", None),
            Some((ChecksumAlgorithm::Sha512, sha512))
        );
        assert_eq!(
            parse_publisher_checksum(
                &format!("{sha256}\n"),
                "image.iso",
                Some(ChecksumAlgorithm::Sha256)
            ),
            Some((ChecksumAlgorithm::Sha256, sha256))
        );
        assert!(parse_publisher_checksum("not-a-digest image.iso", "image.iso", None).is_none());
    }

    #[test]
    fn direct_sidecars_are_bound_to_only_their_iso() {
        let base = Url::parse("https://downloads.example.com/releases/").expect("base URL");
        let (releases, _, _) = parse_download_page(
            r#"
              <a href="one.iso">one</a><a href="two.iso">two</a>
              <a href="one.iso.sha256">one checksum</a>
            "#,
            &base,
        )
        .expect("download page");
        let one = releases
            .iter()
            .find(|release| release.name == "one.iso")
            .expect("one");
        let two = releases
            .iter()
            .find(|release| release.name == "two.iso")
            .expect("two");
        assert_eq!(
            one.checksum_url.as_deref(),
            Some("https://downloads.example.com/releases/one.iso.sha256")
        );
        assert!(two.checksum_url.is_none());
    }

    #[test]
    fn resolves_iso_urls_embedded_in_download_actions() {
        let html = r#"
          <input type="button" value="Download"
            onclick="location.href='https://iso.example.com/linux_1.iso'">
        "#;
        let releases = parse_iso_links(
            html,
            &Url::parse("https://example.com/download").expect("base URL"),
        )
        .expect("embedded ISO");
        assert_eq!(releases[0].name, "linux_1.iso");
    }

    #[test]
    fn collapses_mirrors_with_the_same_iso_filename() {
        let html = r#"
          <a href="https://one.example.com/linux.iso">one</a>
          <a href="https://two.example.com/linux.iso">two</a>
        "#;
        let releases = parse_iso_links(
            html,
            &Url::parse("https://example.com/download").expect("base URL"),
        )
        .expect("mirrors");
        assert_eq!(releases.len(), 1);
    }
}
