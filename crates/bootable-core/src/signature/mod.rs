//! Publisher signature verification for checksum manifests.
//!
//! A checksum fetched from the same server as the image proves little: whoever
//! can alter the image can alter the checksum. A signature made by a key that
//! is pinned in this repository (see [`pinned`]) is real integrity evidence.
//!
//! Outcomes are deliberately three-valued:
//!
//! * [`Verification::Verified`]: a pinned, unexpired, unrevoked key made a
//!   valid signature over exactly these bytes.
//! * [`Verification::Unverified`]: nothing is claimed. The signature is
//!   missing, unreadable, made by an unknown key, or made by a pinned key that
//!   is expired or revoked. Callers fall back to checksum-only integrity and
//!   must say so truthfully.
//! * [`Verification::Rejected`]: a pinned key is named as the signer but the
//!   signature does not verify (tampered manifest or forged signature). The
//!   download must be refused.

pub(crate) mod pinned;

use std::io::Cursor;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use pgp::composed::{CleartextSignedMessage, Deserializable, DetachedSignature, SignedPublicKey};
use pgp::packet::{Signature, SignatureType};
use pgp::types::KeyDetails;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{Error, Result};

const CLEARTEXT_HEADER: &str = "-----BEGIN PGP SIGNED MESSAGE-----";
const MINISIGN_HEADER: &str = "untrusted comment:";
/// Detached-signature file names publishers use next to a checksum manifest.
const OPENPGP_SUFFIXES: [&str; 4] = [".gpg", ".sign", ".asc", ".sig"];
const MINISIGN_SUFFIX: &str = ".minisig";
/// Upper bound on a signature file; real ones are a few hundred bytes.
pub(crate) const MAX_SIGNATURE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureProtocol {
    OpenPgp,
    Minisign,
}

/// The pinned key that produced a verified signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignerIdentity {
    pub publisher: String,
    /// Uppercase hexadecimal v4 fingerprint (OpenPGP) or the base64 public
    /// key (minisign).
    pub fingerprint: String,
    pub protocol: SignatureProtocol,
}

impl SignerIdentity {
    /// Human-sized key identifier: the long key ID (last 16 hex digits) in
    /// groups of four for OpenPGP, the whole public key for minisign.
    pub fn short_fingerprint(&self) -> String {
        match self.protocol {
            SignatureProtocol::OpenPgp => {
                let start = self.fingerprint.len().saturating_sub(16);
                self.fingerprint.as_bytes()[start..]
                    .chunks(4)
                    .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            SignatureProtocol::Minisign => self.fingerprint.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verification {
    Verified(SignerIdentity),
    Unverified(String),
    Rejected(String),
}

struct OpenPgpAnchor {
    publisher: String,
    fingerprint: String,
    key: SignedPublicKey,
    expires_at: Option<SystemTime>,
    revoked: bool,
}

struct MinisignAnchor {
    publisher: String,
    public_key: String,
    key: minisign_verify::PublicKey,
}

/// The set of keys a signature may be checked against.
pub(crate) struct TrustAnchors {
    openpgp: Vec<OpenPgpAnchor>,
    minisign: Vec<MinisignAnchor>,
    /// Ids of pinned entries that failed their self-check and were dropped.
    dropped: Vec<String>,
}

impl TrustAnchors {
    /// The keys pinned in this repository, parsed once.
    pub(crate) fn pinned() -> &'static TrustAnchors {
        static ANCHORS: OnceLock<TrustAnchors> = OnceLock::new();
        ANCHORS.get_or_init(|| {
            let mut anchors = TrustAnchors::empty();
            for entry in pinned::OPENPGP_KEYS {
                if anchors
                    .add_openpgp(entry.publisher, entry.fingerprint, entry.armored)
                    .is_err()
                {
                    anchors.dropped.push(entry.id.to_owned());
                }
            }
            for entry in pinned::MINISIGN_KEYS {
                if anchors
                    .add_minisign(entry.publisher, entry.public_key)
                    .is_err()
                {
                    anchors.dropped.push(entry.id.to_owned());
                }
            }
            anchors
        })
    }

    pub(crate) fn empty() -> Self {
        Self {
            openpgp: Vec::new(),
            minisign: Vec::new(),
            dropped: Vec::new(),
        }
    }

    /// Pin an armored OpenPGP key. The key must hash to `fingerprint`.
    pub(crate) fn add_openpgp(
        &mut self,
        publisher: &str,
        fingerprint: &str,
        armored: &str,
    ) -> std::result::Result<(), String> {
        let (key, _) = SignedPublicKey::from_armor_single(Cursor::new(armored.as_bytes()))
            .map_err(|error| format!("unreadable key: {error}"))?;
        let actual = format!("{:X}", key.fingerprint());
        let expected = fingerprint.replace(' ', "").to_ascii_uppercase();
        if actual != expected {
            return Err(format!(
                "key fingerprint {actual} does not match pinned {expected}"
            ));
        }
        let revoked = !key.details.revocation_signatures.is_empty();
        let expires_at = key_expiry(&key);
        self.openpgp.push(OpenPgpAnchor {
            publisher: publisher.to_owned(),
            fingerprint: actual,
            key,
            expires_at,
            revoked,
        });
        Ok(())
    }

    /// Pin a minisign public key given as its base64 line.
    pub(crate) fn add_minisign(
        &mut self,
        publisher: &str,
        public_key: &str,
    ) -> std::result::Result<(), String> {
        let key = minisign_verify::PublicKey::from_base64(public_key.trim())
            .map_err(|error| format!("unreadable minisign key: {error}"))?;
        self.minisign.push(MinisignAnchor {
            publisher: publisher.to_owned(),
            public_key: public_key.trim().to_owned(),
            key,
        });
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn dropped(&self) -> &[String] {
        &self.dropped
    }

    #[cfg(test)]
    pub(crate) fn openpgp_count(&self) -> usize {
        self.openpgp.len()
    }
}

/// Effective expiry of a primary key from its newest self-signature.
fn key_expiry(key: &SignedPublicKey) -> Option<SystemTime> {
    let fingerprint = key.fingerprint();
    let key_id = key.legacy_key_id();
    let newest = key
        .details
        .direct_signatures
        .iter()
        .chain(key.details.users.iter().flat_map(|user| &user.signatures))
        .filter(|signature| {
            let fingerprints = signature.issuer_fingerprint();
            let key_ids = signature.issuer_key_id();
            if fingerprints.is_empty() && key_ids.is_empty() {
                return true;
            }
            fingerprints.contains(&&fingerprint) || key_ids.contains(&&key_id)
        })
        .filter_map(|signature| Some((signature.created()?, signature)))
        .max_by_key(|(created, _)| created.as_secs())?
        .1;
    let lifetime: Duration = newest.key_expiration_time()?.into();
    if lifetime.is_zero() {
        return None;
    }
    Some(SystemTime::from(key.created_at()) + lifetime)
}

/// Check one OpenPGP signature packet over `data` against the pinned keys.
fn verify_packet(
    anchors: &TrustAnchors,
    signature: &Signature,
    data: &[u8],
    now: SystemTime,
) -> Verification {
    if !matches!(
        signature.typ(),
        Some(SignatureType::Binary | SignatureType::Text)
    ) {
        return Verification::Unverified("the signature is not a document signature".into());
    }
    let fingerprints = signature.issuer_fingerprint();
    let key_ids = signature.issuer_key_id();
    let names_issuer = !fingerprints.is_empty() || !key_ids.is_empty();
    let candidates = anchors
        .openpgp
        .iter()
        .filter(|anchor| {
            !names_issuer
                || fingerprints.contains(&&anchor.key.fingerprint())
                || key_ids.contains(&&anchor.key.legacy_key_id())
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Verification::Unverified(
            "the signature was made by a key Bootable does not pin".into(),
        );
    }
    if let (Some(created), Some(lifetime)) =
        (signature.created(), signature.signature_expiration_time())
    {
        let lifetime: Duration = lifetime.into();
        if !lifetime.is_zero() && SystemTime::from(created) + lifetime < now {
            return Verification::Unverified("the signature itself has expired".into());
        }
    }
    let mut notes = Vec::new();
    let mut failed = false;
    for anchor in candidates {
        if anchor.revoked {
            notes.push(format!("pinned {} key is revoked", anchor.publisher));
            continue;
        }
        if anchor.expires_at.is_some_and(|expiry| expiry <= now) {
            notes.push(format!("pinned {} key has expired", anchor.publisher));
            continue;
        }
        match signature.verify(&anchor.key, data) {
            Ok(()) => {
                return Verification::Verified(SignerIdentity {
                    publisher: anchor.publisher.clone(),
                    fingerprint: anchor.fingerprint.clone(),
                    protocol: SignatureProtocol::OpenPgp,
                });
            }
            Err(_) => failed = true,
        }
    }
    if failed && names_issuer {
        Verification::Rejected(
            "the signature does not match the manifest for the key it names".into(),
        )
    } else if failed {
        Verification::Unverified("the signature does not match any pinned key".into())
    } else {
        Verification::Unverified(notes.join("; "))
    }
}

/// Verify a binary or armored OpenPGP detached signature over `data`.
pub(crate) fn verify_openpgp_detached(
    anchors: &TrustAnchors,
    data: &[u8],
    signature: &[u8],
    now: SystemTime,
) -> Verification {
    let parsed = match DetachedSignature::from_reader_single(Cursor::new(signature)) {
        Ok((parsed, _)) => parsed,
        Err(_) => return Verification::Unverified("the signature file is unreadable".into()),
    };
    verify_packet(anchors, &parsed.signature, data, now)
}

/// Verify an OpenPGP cleartext-signed document. On success the authenticated
/// text is returned; anything outside the signed block is discarded.
pub(crate) fn verify_openpgp_cleartext(
    anchors: &TrustAnchors,
    document: &str,
    now: SystemTime,
) -> (Verification, Option<String>) {
    let message = match CleartextSignedMessage::from_string(document) {
        Ok((message, _)) => message,
        Err(_) => {
            return (
                Verification::Unverified("the signed checksum file is unreadable".into()),
                None,
            );
        }
    };
    let signed_text = message.signed_text();
    let mut verified = None;
    let mut first_unverified = None;
    for signature in message.signatures() {
        match verify_packet(anchors, signature, signed_text.as_bytes(), now) {
            Verification::Rejected(reason) => return (Verification::Rejected(reason), None),
            Verification::Verified(signer) => verified = verified.or(Some(signer)),
            Verification::Unverified(reason) => {
                first_unverified = first_unverified.or(Some(reason));
            }
        }
    }
    match verified {
        Some(signer) => (Verification::Verified(signer), Some(signed_text)),
        None => (
            Verification::Unverified(
                first_unverified.unwrap_or_else(|| "the checksum file carries no signature".into()),
            ),
            None,
        ),
    }
}

/// Verify a minisign signature (prehashed `ED` mode only) over `data`.
pub(crate) fn verify_minisign(
    anchors: &TrustAnchors,
    data: &[u8],
    signature: &str,
) -> Verification {
    let parsed = match minisign_verify::Signature::decode(signature) {
        Ok(parsed) => parsed,
        Err(_) => return Verification::Unverified("the minisign signature is unreadable".into()),
    };
    let mut legacy = false;
    let mut failed = false;
    for anchor in &anchors.minisign {
        match anchor.key.verify(data, &parsed, false) {
            Ok(()) => {
                return Verification::Verified(SignerIdentity {
                    publisher: anchor.publisher.clone(),
                    fingerprint: anchor.public_key.clone(),
                    protocol: SignatureProtocol::Minisign,
                });
            }
            Err(minisign_verify::Error::UnexpectedKeyId) => {}
            Err(minisign_verify::Error::UnexpectedAlgorithm) => legacy = true,
            Err(_) => failed = true,
        }
    }
    if failed {
        Verification::Rejected(
            "the minisign signature does not match the manifest for the key it names".into(),
        )
    } else if legacy {
        Verification::Unverified(
            "legacy (non-prehashed) minisign signatures are not accepted".into(),
        )
    } else {
        Verification::Unverified("the signature was made by a key Bootable does not pin".into())
    }
}

/// What is known about the authenticity of a fetched checksum manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManifestSignature {
    Verified(SignerIdentity),
    Unverified(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthenticatedManifest {
    /// The text the checksum must be read from. When the signature verified
    /// this is exactly the signed text.
    pub text: String,
    pub signature: ManifestSignature,
}

/// Sibling URL of the manifest with `suffix` appended to its file name.
fn signature_url(manifest: &Url, suffix: &str) -> Option<Url> {
    let mut url = manifest.clone();
    let name = manifest.path_segments()?.next_back()?;
    if name.is_empty() {
        return None;
    }
    url.set_path(&format!("{}{suffix}", manifest.path()));
    url.set_query(None);
    url.set_fragment(None);
    Some(url)
}

/// Authenticate a checksum manifest against pinned keys.
///
/// `fetch` returns the body of an optional sibling signature file, or `None`
/// when it is absent or unreachable. It is only called for signature types for
/// which at least one pinned key exists. A [`Verification::Rejected`] outcome
/// becomes an error that refuses the download.
pub(crate) fn authenticate_manifest(
    anchors: &TrustAnchors,
    manifest_url: &Url,
    document: &[u8],
    now: SystemTime,
    mut fetch: impl FnMut(&Url) -> Option<Vec<u8>>,
) -> Result<AuthenticatedManifest> {
    let text = String::from_utf8(document.to_vec())
        .map_err(|error| Error::InvalidCatalog(format!("catalog is not UTF-8: {error}")))?;
    let refuse = |reason: String| {
        Error::InvalidDownload(format!(
            "signature verification failed for {manifest_url}: {reason}"
        ))
    };

    if text.trim_start().starts_with(CLEARTEXT_HEADER) {
        let (verification, signed) = verify_openpgp_cleartext(anchors, &text, now);
        return match verification {
            Verification::Verified(signer) => Ok(AuthenticatedManifest {
                text: signed.unwrap_or(text),
                signature: ManifestSignature::Verified(signer),
            }),
            Verification::Unverified(reason) => Ok(AuthenticatedManifest {
                text,
                signature: ManifestSignature::Unverified(reason),
            }),
            Verification::Rejected(reason) => Err(refuse(reason)),
        };
    }

    let mut suffixes = Vec::new();
    if !anchors.openpgp.is_empty() {
        suffixes.extend(OPENPGP_SUFFIXES);
    }
    if !anchors.minisign.is_empty() {
        suffixes.push(MINISIGN_SUFFIX);
    }
    let mut reason = None;
    for suffix in suffixes {
        let Some(url) = signature_url(manifest_url, suffix) else {
            continue;
        };
        let Some(body) = fetch(&url) else { continue };
        let verification = match std::str::from_utf8(&body) {
            Ok(minisign) if minisign.trim_start().starts_with(MINISIGN_HEADER) => {
                verify_minisign(anchors, document, minisign)
            }
            _ => verify_openpgp_detached(anchors, document, &body, now),
        };
        match verification {
            Verification::Verified(signer) => {
                return Ok(AuthenticatedManifest {
                    text,
                    signature: ManifestSignature::Verified(signer),
                });
            }
            Verification::Rejected(reason) => return Err(refuse(reason)),
            Verification::Unverified(why) => reason = reason.or(Some(why)),
        }
    }
    Ok(AuthenticatedManifest {
        text,
        signature: ManifestSignature::Unverified(reason.unwrap_or_else(|| {
            "the publisher does not publish a signature next to the checksum file".into()
        })),
    })
}

#[cfg(test)]
mod tests;
