use crate::{CatalogFetch, CatalogState};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiscoverySource {
    #[default]
    DistroWatch,
    RaspberryPi,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QuickAccess {
    #[default]
    All,
    Arch,
    Debian,
    Omarchy,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogFacet {
    Popular,
    Directory,
    RaspberryPi,
    Arch,
    Debian,
    Details,
}

/// Shared discovery state and stale-response gate for all presentation adapters.
#[derive(Debug, Default)]
pub struct DiscoverySession {
    source: DiscoverySource,
    quick_access: QuickAccess,
    popular: CatalogState,
    directory: CatalogState,
    raspberry_pi: CatalogState,
    arch: CatalogState,
    debian: CatalogState,
    details: CatalogState,
    expected_details_slug: Option<String>,
}

impl DiscoverySession {
    pub fn source(&self) -> DiscoverySource {
        self.source
    }

    pub fn quick_access(&self) -> QuickAccess {
        self.quick_access
    }

    pub fn show_distrowatch(&mut self, preset: QuickAccess) {
        self.source = DiscoverySource::DistroWatch;
        self.quick_access = preset;
    }

    pub fn show_raspberry_pi(&mut self) {
        self.source = DiscoverySource::RaspberryPi;
        self.quick_access = QuickAccess::All;
        self.clear_details();
    }

    pub fn state(&self, facet: CatalogFacet) -> &CatalogState {
        match facet {
            CatalogFacet::Popular => &self.popular,
            CatalogFacet::Directory => &self.directory,
            CatalogFacet::RaspberryPi => &self.raspberry_pi,
            CatalogFacet::Arch => &self.arch,
            CatalogFacet::Debian => &self.debian,
            CatalogFacet::Details => &self.details,
        }
    }

    /// Begins an idempotent load. Returns false when the same facet is already loading.
    pub fn begin(&mut self, facet: CatalogFacet) -> bool {
        if self.state(facet).is_loading() {
            return false;
        }
        *self.state_mut(facet) = CatalogState::Loading;
        true
    }

    pub fn complete<T>(&mut self, facet: CatalogFacet, fetch: &CatalogFetch<T>, empty: bool) {
        *self.state_mut(facet) = CatalogState::from_fetch(fetch, empty);
    }

    pub fn fail(&mut self, facet: CatalogFacet, error: impl Into<String>) {
        *self.state_mut(facet) = CatalogState::Failed(error.into());
    }

    pub fn expect_details(&mut self, slug: impl Into<String>) {
        self.expected_details_slug = Some(slug.into());
        self.details = CatalogState::Loading;
    }

    pub fn accepts_details(&self, slug: &str) -> bool {
        self.expected_details_slug.as_deref() == Some(slug)
    }

    pub fn clear_details(&mut self) {
        self.expected_details_slug = None;
        self.details = CatalogState::Idle;
    }

    fn state_mut(&mut self, facet: CatalogFacet) -> &mut CatalogState {
        match facet {
            CatalogFacet::Popular => &mut self.popular,
            CatalogFacet::Directory => &mut self.directory,
            CatalogFacet::RaspberryPi => &mut self.raspberry_pi,
            CatalogFacet::Arch => &mut self.arch,
            CatalogFacet::Debian => &mut self.debian,
            CatalogFacet::Details => &mut self.details,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CatalogOrigin;

    #[test]
    fn source_and_preset_change_atomically() {
        let mut session = DiscoverySession::default();
        session.show_raspberry_pi();
        assert_eq!(session.source(), DiscoverySource::RaspberryPi);
        assert_eq!(session.quick_access(), QuickAccess::All);
        session.show_distrowatch(QuickAccess::Arch);
        assert_eq!(session.source(), DiscoverySource::DistroWatch);
        assert_eq!(session.quick_access(), QuickAccess::Arch);
    }

    #[test]
    fn duplicate_loads_are_suppressed_and_completion_is_structured() {
        let mut session = DiscoverySession::default();
        assert!(session.begin(CatalogFacet::Popular));
        assert!(!session.begin(CatalogFacet::Popular));
        let fetch = CatalogFetch {
            value: vec!["one"],
            origin: CatalogOrigin::Network,
            age: None,
            warning: None,
        };
        session.complete(CatalogFacet::Popular, &fetch, false);
        assert!(matches!(
            session.state(CatalogFacet::Popular),
            CatalogState::Ready { .. }
        ));
    }

    #[test]
    fn stale_distribution_response_is_rejected() {
        let mut session = DiscoverySession::default();
        session.expect_details("arch");
        session.expect_details("debian");
        assert!(!session.accepts_details("arch"));
        assert!(session.accepts_details("debian"));
    }
}
