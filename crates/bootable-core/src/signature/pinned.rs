//! Pinned publisher signing keys.
//!
//! This file is the entire trust root for signature verification. A signature
//! only upgrades a download to "signature verified" when it was made by one of
//! the keys listed here. Rules (see `docs/signatures.md`):
//!
//! * Every entry carries the full 40-hex v4 fingerprint, copied from the
//!   publisher's own documentation (the provenance URL in the comment).
//! * The armored key under `keys/` is fetched separately (publisher URL or the
//!   Ubuntu keyserver), reduced with `gpg --export-options export-minimal`, and
//!   must hash to exactly the pinned fingerprint. `TrustAnchors::pinned` re-checks
//!   this at runtime and drops any entry that does not match, and a unit test
//!   fails the build if any entry is dropped.
//! * Only the primary key is trusted to sign (all publishers below sign with
//!   the primary key). Subkey-signed manifests are not accepted.
//! * Keys that are expired or carry a revocation signature are ignored.
//!
//! Do not add a key whose fingerprint you could not confirm from an
//! authoritative publisher page.

/// An OpenPGP signing key pinned by fingerprint.
pub(crate) struct PinnedOpenPgpKey {
    /// Stable identifier, used in tests and documentation.
    pub id: &'static str,
    /// Short human label shown in the integrity label.
    pub publisher: &'static str,
    /// Uppercase hexadecimal v4 fingerprint, no spaces.
    pub fingerprint: &'static str,
    /// ASCII-armored public key.
    pub armored: &'static str,
}

/// A minisign public key pinned by its base64 public-key line.
pub(crate) struct PinnedMinisignKey {
    pub id: &'static str,
    pub publisher: &'static str,
    /// The base64 public key line from the publisher's `minisign.pub`.
    pub public_key: &'static str,
}

pub(crate) const OPENPGP_KEYS: &[PinnedOpenPgpKey] = &[
    // Ubuntu CD Image Automatic Signing Key (2012) <cdimage@ubuntu.com>
    // Signs SHA256SUMS -> SHA256SUMS.gpg for releases.ubuntu.com, cdimage.ubuntu.com
    // and the flavours.
    // Fingerprint: https://ubuntu.com/tutorials/how-to-verify-ubuntu
    // Key: https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x843938DF228D22F7B3742BC0D94AA3F0EFE21092
    // The legacy 1024-bit DSA key (C598 6B4F 1257 FFA8 6632 CBA7 4618 1433 FBB7 5451)
    // is intentionally not pinned.
    PinnedOpenPgpKey {
        id: "ubuntu-cdimage-2012",
        publisher: "Ubuntu",
        fingerprint: "843938DF228D22F7B3742BC0D94AA3F0EFE21092",
        armored: include_str!("../../keys/ubuntu-cdimage-2012.asc"),
    },
    // Debian CD signing key <debian-cd@lists.debian.org>, created 2009-10-03.
    // Signs SHA256SUMS/SHA512SUMS -> *.sign on cdimage.debian.org.
    // Fingerprint: https://www.debian.org/CD/verify
    // Key: https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x988021A964E6EA7D
    PinnedOpenPgpKey {
        id: "debian-cd-2009",
        publisher: "Debian CD",
        fingerprint: "10460DAD76165AD81FBC0CE9988021A964E6EA7D",
        armored: include_str!("../../keys/debian-cd-2009.asc"),
    },
    // Debian CD signing key <debian-cd@lists.debian.org>, created 2011-01-05.
    // Signs the current stable images (SHA256SUMS.sign).
    // Fingerprint: https://www.debian.org/CD/verify
    // Key: https://keyserver.ubuntu.com/pks/lookup?op=get&search=0xDA87E80D6294BE9B
    PinnedOpenPgpKey {
        id: "debian-cd-2011",
        publisher: "Debian CD",
        fingerprint: "DF9B9C49EAA9298432589D76DA87E80D6294BE9B",
        armored: include_str!("../../keys/debian-cd-2011.asc"),
    },
    // Debian Testing CDs Automatic Signing Key <debian-cd@lists.debian.org>, created 2014-04-15.
    // Fingerprint: https://www.debian.org/CD/verify
    // Key: https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x42468F4009EA8AC3
    PinnedOpenPgpKey {
        id: "debian-cd-testing-2014",
        publisher: "Debian Testing CD",
        fingerprint: "F41D30342F3546695F65C66942468F4009EA8AC3",
        armored: include_str!("../../keys/debian-cd-testing-2014.asc"),
    },
    // Linux Mint ISO Signing Key <root@linuxmint.com>
    // Signs sha256sum.txt -> sha256sum.txt.gpg next to each Mint ISO (and every mirror).
    // Fingerprint: https://linuxmint-installation-guide.readthedocs.io/en/latest/verify.html
    // Key: https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x27DEB15644C6B3CF3BD7D291300F846BA25BAE09
    PinnedOpenPgpKey {
        id: "linuxmint-iso",
        publisher: "Linux Mint",
        fingerprint: "27DEB15644C6B3CF3BD7D291300F846BA25BAE09",
        armored: include_str!("../../keys/linuxmint-iso.asc"),
    },
    // Kali Linux Archive Automatic Signing Key (2025) <devel@kali.org>, expires 2028-04-17.
    // Signs SHA256SUMS -> SHA256SUMS.gpg on cdimage.kali.org.
    // Fingerprint: https://www.kali.org/docs/introduction/download-images-securely/
    // Key: https://archive.kali.org/archive-key.asc
    PinnedOpenPgpKey {
        id: "kali-archive-2025",
        publisher: "Kali Linux",
        fingerprint: "827C8569F2518CC677FECA1AED65462EC8D5E4C5",
        armored: include_str!("../../keys/kali-archive-2025.asc"),
    },
    // Fedora release signing keys. Each Fedora release signs its own clear-signed
    // *-CHECKSUM files with the primary key of that release.
    // Fingerprints: https://fedoraproject.org/security
    // Keys: https://fedoraproject.org/fedora.gpg (42 from the Ubuntu keyserver)
    PinnedOpenPgpKey {
        id: "fedora-42",
        publisher: "Fedora 42",
        fingerprint: "B0F4950458F69E1150C6C5EDC8AC4916105EF944",
        armored: include_str!("../../keys/fedora-42.asc"),
    },
    PinnedOpenPgpKey {
        id: "fedora-43",
        publisher: "Fedora 43",
        fingerprint: "C6E7F081CF80E13146676E88829B606631645531",
        armored: include_str!("../../keys/fedora-43.asc"),
    },
    PinnedOpenPgpKey {
        id: "fedora-44",
        publisher: "Fedora 44",
        fingerprint: "36F612DCF27F7D1A48A835E4DBFCF71C6D9F90A6",
        armored: include_str!("../../keys/fedora-44.asc"),
    },
    PinnedOpenPgpKey {
        id: "fedora-45",
        publisher: "Fedora 45",
        fingerprint: "4F50A6114CD5C6976A7F1179655A4B02F577861E",
        armored: include_str!("../../keys/fedora-45.asc"),
    },
    // AlmaLinux OS release keys. They sign the clear-signed CHECKSUM files next to the ISOs
    // (confirmed against repo.almalinux.org/almalinux/{9,10}/isos/x86_64/CHECKSUM).
    // Fingerprints: https://almalinux.org/security/
    // Keys: https://repo.almalinux.org/almalinux/RPM-GPG-KEY-AlmaLinux-{9,10}
    PinnedOpenPgpKey {
        id: "almalinux-9",
        publisher: "AlmaLinux 9",
        fingerprint: "BF18AC2876178908D6E71267D36CB86CB86B3716",
        armored: include_str!("../../keys/almalinux-9.asc"),
    },
    PinnedOpenPgpKey {
        id: "almalinux-10",
        publisher: "AlmaLinux 10",
        fingerprint: "EE6DB7B98F5BF5EDD9DA0DE5DEE5C11CC2A1E572",
        armored: include_str!("../../keys/almalinux-10.asc"),
    },
];

/// No catalog publisher whose minisign key could be confirmed from an
/// authoritative page is pinned yet. The verification path is implemented and
/// tested with generated keys; see `docs/signatures.md` for how to add one.
pub(crate) const MINISIGN_KEYS: &[PinnedMinisignKey] = &[];

/// Where a pinned publisher is known to sign its checksum manifests. A
/// manifest fetched from a matching URL is *expected* to carry a verifiable
/// signature, so a missing or unusable one is reported as a downgrade rather
/// than as an ordinary unsigned publisher. Matching is by host suffix (the
/// publisher's own domains) or by a path segment that official mirrors keep
/// (`/linuxmint/`). Arbitrary third-party mirrors are not listed: for those the
/// absence of a signature stays an ordinary checksum-only result. Expecting a
/// signature can only make a result more cautious, so a spoofed match is harmless.
pub(crate) struct SignedManifestSource {
    pub publisher: &'static str,
    /// Host equals the entry or ends with `.` + the entry.
    pub host_suffixes: &'static [&'static str],
    /// URL path contains the entry (for well-known mirror layouts).
    pub path_contains: &'static [&'static str],
}

pub(crate) const SIGNED_MANIFEST_SOURCES: &[SignedManifestSource] = &[
    SignedManifestSource {
        publisher: "Ubuntu",
        host_suffixes: &["ubuntu.com"],
        path_contains: &[],
    },
    SignedManifestSource {
        publisher: "Debian",
        host_suffixes: &["debian.org"],
        path_contains: &[],
    },
    SignedManifestSource {
        publisher: "Linux Mint",
        host_suffixes: &["linuxmint.com"],
        path_contains: &["/linuxmint/"],
    },
    SignedManifestSource {
        publisher: "Kali Linux",
        host_suffixes: &["kali.org"],
        path_contains: &[],
    },
    SignedManifestSource {
        publisher: "Fedora",
        host_suffixes: &["fedoraproject.org"],
        path_contains: &[],
    },
    SignedManifestSource {
        publisher: "AlmaLinux",
        host_suffixes: &["almalinux.org"],
        path_contains: &[],
    },
];

/// The publisher that is known to sign manifests served from `url`, if any.
pub(crate) fn signing_publisher_for(url: &url::Url) -> Option<&'static str> {
    let host = url.host_str()?.to_ascii_lowercase();
    let path = url.path().to_ascii_lowercase();
    SIGNED_MANIFEST_SOURCES
        .iter()
        .find(|source| {
            source.host_suffixes.iter().any(|suffix| {
                host == *suffix
                    || host
                        .strip_suffix(suffix)
                        .is_some_and(|rest| rest.ends_with('.'))
            }) || source
                .path_contains
                .iter()
                .any(|fragment| path.contains(fragment))
        })
        .map(|source| source.publisher)
}
