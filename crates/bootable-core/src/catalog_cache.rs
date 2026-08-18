use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result, io_error};

const CACHE_VERSION: u32 = 1;
pub(crate) const CATALOG_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CacheMode {
    #[default]
    PreferCache,
    Refresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogOrigin {
    Network,
    FreshCache,
    StaleCache,
}

#[derive(Debug, Clone)]
pub struct CatalogFetch<T> {
    pub value: T,
    pub origin: CatalogOrigin,
    pub age: Option<Duration>,
    pub warning: Option<String>,
}

impl<T> CatalogFetch<T> {
    pub fn source_label(&self) -> &'static str {
        match self.origin {
            CatalogOrigin::Network => "updated now",
            CatalogOrigin::FreshCache => "cached",
            CatalogOrigin::StaleCache => "cached · refresh failed",
        }
    }

    pub fn status_suffix(&self) -> String {
        match &self.warning {
            Some(warning) => format!("{} · {warning}", self.source_label()),
            None => self.source_label().into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CatalogState {
    #[default]
    Idle,
    Loading,
    Ready {
        origin: CatalogOrigin,
        warning: Option<String>,
    },
    Empty,
    Failed(String),
}

impl CatalogState {
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    pub fn from_fetch<T>(fetch: &CatalogFetch<T>, empty: bool) -> Self {
        if empty {
            Self::Empty
        } else {
            Self::Ready {
                origin: fetch.origin,
                warning: fetch.warning.clone(),
            }
        }
    }

    pub fn short_label(&self, subject: &str) -> String {
        match self {
            Self::Idle => format!("{subject} not loaded"),
            Self::Loading => format!("Loading {subject}…"),
            Self::Ready {
                origin: CatalogOrigin::Network,
                warning: None,
            } => format!("{subject} ready"),
            Self::Ready {
                origin: CatalogOrigin::FreshCache,
                warning: None,
            } => format!("{subject} ready · cached"),
            Self::Ready { warning, .. } => warning.as_ref().map_or_else(
                || format!("{subject} ready · cached"),
                |warning| format!("{subject} ready · cached · {warning}"),
            ),
            Self::Empty => format!("No {subject} found"),
            Self::Failed(message) => format!("Could not load {subject} · {message} · retry"),
        }
    }
}

#[derive(Serialize, serde::Deserialize)]
struct CacheEnvelope<T> {
    version: u32,
    saved_at: u64,
    value: T,
}

pub(crate) fn load_or_fetch<T>(
    key: &str,
    mode: CacheMode,
    fetch: impl FnOnce() -> Result<T>,
) -> Result<CatalogFetch<T>>
where
    T: Clone + Serialize + DeserializeOwned,
{
    load_or_fetch_in(&cache_root(), key, CATALOG_TTL, mode, fetch)
}

fn load_or_fetch_in<T>(
    root: &Path,
    key: &str,
    ttl: Duration,
    mode: CacheMode,
    fetch: impl FnOnce() -> Result<T>,
) -> Result<CatalogFetch<T>>
where
    T: Clone + Serialize + DeserializeOwned,
{
    validate_key(key)?;
    let path = root.join(format!("{key}.json"));
    let (cached, cache_warning) = match read_cache::<T>(&path) {
        Ok(cached) => (cached, None),
        Err(error) => (None, Some(format!("cache ignored: {error}"))),
    };
    if mode == CacheMode::PreferCache
        && let Some((value, age)) = cached.as_ref()
        && *age <= ttl
    {
        return Ok(CatalogFetch {
            value: value.clone(),
            origin: CatalogOrigin::FreshCache,
            age: Some(*age),
            warning: cache_warning,
        });
    }

    match fetch() {
        Ok(value) => {
            let warning = write_cache(root, &path, &value)
                .err()
                .map(|error| format!("cache unavailable: {error}"));
            Ok(CatalogFetch {
                value,
                origin: CatalogOrigin::Network,
                age: None,
                warning: warning.or(cache_warning),
            })
        }
        Err(error) => {
            if let Some((value, age)) = cached {
                return Ok(CatalogFetch {
                    value,
                    origin: CatalogOrigin::StaleCache,
                    age: Some(age),
                    warning: Some(format!("refresh failed: {error}")),
                });
            }
            Err(match cache_warning {
                Some(cache_error) => Error::InvalidCatalog(format!(
                    "{error}; cached data was unusable ({cache_error})"
                )),
                None => error,
            })
        }
    }
}

fn cache_root() -> PathBuf {
    std::env::temp_dir()
        .join("bootable")
        .join("catalog-cache-v1")
}

fn validate_key(key: &str) -> Result<()> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::InvalidCatalog("invalid cache key".into()));
    }
    Ok(())
}

fn read_cache<T: DeserializeOwned>(path: &Path) -> Result<Option<(T, Duration)>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(path, error)),
    };
    let envelope: CacheEnvelope<T> = serde_json::from_slice(&bytes).map_err(|error| {
        Error::InvalidCatalog(format!("invalid cache at {}: {error}", path.display()))
    })?;
    if envelope.version != CACHE_VERSION {
        return Ok(None);
    }
    let now = unix_seconds()?;
    let age = Duration::from_secs(now.saturating_sub(envelope.saved_at));
    Ok(Some((envelope.value, age)))
}

fn write_cache<T: Serialize>(root: &Path, path: &Path, value: &T) -> Result<()> {
    fs::create_dir_all(root).map_err(|error| io_error(root, error))?;
    let envelope = CacheEnvelope {
        version: CACHE_VERSION,
        saved_at: unix_seconds()?,
        value,
    };
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|error| Error::InvalidCatalog(format!("serialize cache: {error}")))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".catalog-")
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

fn unix_seconds() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| Error::InvalidCatalog(format!("system clock before Unix epoch: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn fresh_cache_skips_the_fetch() {
        let directory = tempfile::tempdir().expect("temp cache");
        let calls = AtomicUsize::new(0);
        let first = load_or_fetch_in(
            directory.path(),
            "popular",
            Duration::from_secs(60),
            CacheMode::PreferCache,
            || Ok(vec!["Omarchy".to_string()]),
        )
        .expect("network result");
        let second = load_or_fetch_in(
            directory.path(),
            "popular",
            Duration::from_secs(60),
            CacheMode::PreferCache,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec!["unexpected".to_string()])
            },
        )
        .expect("cached result");
        assert_eq!(first.origin, CatalogOrigin::Network);
        assert_eq!(second.origin, CatalogOrigin::FreshCache);
        assert_eq!(second.value, vec!["Omarchy"]);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stale_cache_is_used_when_refresh_fails() {
        let directory = tempfile::tempdir().expect("temp cache");
        load_or_fetch_in(
            directory.path(),
            "pi",
            Duration::ZERO,
            CacheMode::PreferCache,
            || Ok(vec![1_u8, 2, 3]),
        )
        .expect("seed cache");
        let fallback: CatalogFetch<Vec<u8>> = load_or_fetch_in(
            directory.path(),
            "pi",
            Duration::ZERO,
            CacheMode::Refresh,
            || {
                Err(Error::Network {
                    url: "https://example.invalid".into(),
                    message: "offline".into(),
                })
            },
        )
        .expect("stale fallback");
        assert_eq!(fallback.origin, CatalogOrigin::StaleCache);
        assert_eq!(fallback.value, vec![1, 2, 3]);
        assert!(
            fallback
                .warning
                .as_deref()
                .is_some_and(|text| text.contains("offline"))
        );
    }

    #[test]
    fn corrupt_cache_does_not_hide_a_successful_refresh() {
        let directory = tempfile::tempdir().expect("temp cache");
        fs::write(directory.path().join("catalog.json"), b"not json").expect("corrupt cache");
        let result = load_or_fetch_in(
            directory.path(),
            "catalog",
            Duration::from_secs(60),
            CacheMode::PreferCache,
            || Ok(vec![42_u8]),
        )
        .expect("network recovery");
        assert_eq!(result.value, vec![42]);
        assert_eq!(result.origin, CatalogOrigin::Network);
        assert!(
            result
                .warning
                .as_deref()
                .is_some_and(|text| text.contains("cache ignored"))
        );
    }

    #[test]
    fn manual_refresh_bypasses_a_fresh_cache() {
        let directory = tempfile::tempdir().expect("temp cache");
        load_or_fetch_in(
            directory.path(),
            "directory",
            Duration::from_secs(60),
            CacheMode::PreferCache,
            || Ok(vec![1_u8]),
        )
        .expect("seed cache");
        let refreshed = load_or_fetch_in(
            directory.path(),
            "directory",
            Duration::from_secs(60),
            CacheMode::Refresh,
            || Ok(vec![2_u8]),
        )
        .expect("refresh");
        assert_eq!(refreshed.origin, CatalogOrigin::Network);
        assert_eq!(refreshed.value, vec![2]);
    }

    #[test]
    fn unsafe_cache_keys_are_rejected() {
        let directory = tempfile::tempdir().expect("temp cache");
        let error = load_or_fetch_in(
            directory.path(),
            "../outside",
            Duration::from_secs(60),
            CacheMode::PreferCache,
            || Ok(vec![1_u8]),
        )
        .expect_err("unsafe key");
        assert!(error.to_string().contains("invalid cache key"));
    }

    #[test]
    fn state_labels_cover_loading_empty_failure_and_stale_data() {
        assert_eq!(
            CatalogState::Loading.short_label("images"),
            "Loading images…"
        );
        assert_eq!(CatalogState::Empty.short_label("images"), "No images found");
        assert!(
            CatalogState::Failed("offline".into())
                .short_label("images")
                .contains("retry")
        );
        assert!(
            CatalogState::Ready {
                origin: CatalogOrigin::StaleCache,
                warning: Some("refresh failed".into()),
            }
            .short_label("images")
            .contains("refresh failed")
        );
    }
}
