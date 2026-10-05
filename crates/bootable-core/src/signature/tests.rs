use std::cell::RefCell;
use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signer, SigningKey};
use pgp::composed::{
    ArmorOptions, CleartextSignedMessage, DetachedSignature, KeyType, SecretKeyParamsBuilder,
    SignedPublicKey, SignedSecretKey,
};
use pgp::crypto::hash::HashAlgorithm;
use pgp::types::{KeyDetails, Password};
use rand::thread_rng;
use url::Url;

use super::*;

/// 2026-10-04, inside the validity window of every pinned key.
fn today() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_791_072_000)
}

/// 2028-06-01, after the Kali signing key expires on 2028-04-17.
fn after_kali_expiry() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_843_430_400)
}

struct Fixture {
    manifest: &'static [u8],
    signature: &'static [u8],
    publisher: &'static str,
    fingerprint: &'static str,
}

const UBUNTU: Fixture = Fixture {
    manifest: include_bytes!("../../testdata/ubuntu-24.04.SHA256SUMS"),
    signature: include_bytes!("../../testdata/ubuntu-24.04.SHA256SUMS.gpg"),
    publisher: "Ubuntu",
    fingerprint: "843938DF228D22F7B3742BC0D94AA3F0EFE21092",
};
const DEBIAN: Fixture = Fixture {
    manifest: include_bytes!("../../testdata/debian.SHA256SUMS"),
    signature: include_bytes!("../../testdata/debian.SHA256SUMS.sign"),
    publisher: "Debian CD",
    fingerprint: "DF9B9C49EAA9298432589D76DA87E80D6294BE9B",
};
const MINT: Fixture = Fixture {
    manifest: include_bytes!("../../testdata/mint-22.3.sha256sum.txt"),
    signature: include_bytes!("../../testdata/mint-22.3.sha256sum.txt.gpg"),
    publisher: "Linux Mint",
    fingerprint: "27DEB15644C6B3CF3BD7D291300F846BA25BAE09",
};
const KALI: Fixture = Fixture {
    manifest: include_bytes!("../../testdata/kali.SHA256SUMS"),
    signature: include_bytes!("../../testdata/kali.SHA256SUMS.gpg"),
    publisher: "Kali Linux",
    fingerprint: "827C8569F2518CC677FECA1AED65462EC8D5E4C5",
};
const FEDORA_44: &str = include_str!("../../testdata/fedora-44-workstation.CHECKSUM");
const ALMA_10: &str = include_str!("../../testdata/almalinux-10.CHECKSUM");

fn expect_verified(verification: Verification, publisher: &str, fingerprint: &str) {
    match verification {
        Verification::Verified(signer) => {
            assert_eq!(signer.publisher, publisher);
            assert_eq!(signer.fingerprint, fingerprint);
            assert_eq!(signer.protocol, SignatureProtocol::OpenPgp);
        }
        other => panic!("expected a verified signature, got {other:?}"),
    }
}

fn tampered(bytes: &[u8]) -> Vec<u8> {
    let mut copy = bytes.to_vec();
    let last = copy.len() - 2;
    copy[last] = if copy[last] == b'0' { b'1' } else { b'0' };
    copy
}

// ---------------------------------------------------------------------------
// The pinned key table
// ---------------------------------------------------------------------------

#[test]
fn every_pinned_key_loads_and_matches_its_fingerprint() {
    let anchors = TrustAnchors::pinned();
    assert!(
        anchors.dropped().is_empty(),
        "pinned entries failed their fingerprint self-check: {:?}",
        anchors.dropped()
    );
    assert_eq!(anchors.openpgp_count(), pinned::OPENPGP_KEYS.len());
}

#[test]
fn pinned_table_is_well_formed() {
    let mut ids = HashSet::new();
    let mut fingerprints = HashSet::new();
    for key in pinned::OPENPGP_KEYS {
        assert!(ids.insert(key.id), "duplicate id {}", key.id);
        assert!(
            fingerprints.insert(key.fingerprint),
            "duplicate fingerprint {}",
            key.fingerprint
        );
        assert_eq!(key.fingerprint.len(), 40, "{}", key.id);
        assert!(
            key.fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte)),
            "{} fingerprint must be uppercase hexadecimal",
            key.id
        );
        assert!(!key.publisher.is_empty());
    }
}

#[test]
fn a_key_that_does_not_hash_to_its_pinned_fingerprint_is_refused() {
    let mut anchors = TrustAnchors::empty();
    let error = anchors
        .add_openpgp(
            "Ubuntu",
            "DF9B9C49EAA9298432589D76DA87E80D6294BE9B",
            pinned::OPENPGP_KEYS[0].armored,
        )
        .expect_err("a substituted key must not be pinned");
    assert!(error.contains("does not match"));
    assert_eq!(anchors.openpgp_count(), 0);
}

#[test]
fn garbage_is_not_a_pinnable_key() {
    let mut anchors = TrustAnchors::empty();
    assert!(
        anchors
            .add_openpgp("Nobody", "00".repeat(20).as_str(), "not a key")
            .is_err()
    );
    assert!(anchors.add_minisign("Nobody", "not base64!").is_err());
}

// ---------------------------------------------------------------------------
// Real publisher signatures verify against the pinned keys
// ---------------------------------------------------------------------------

#[test]
fn real_detached_signatures_verify_against_pinned_keys() {
    let anchors = TrustAnchors::pinned();
    for fixture in [&UBUNTU, &DEBIAN, &MINT, &KALI] {
        expect_verified(
            verify_openpgp_detached(anchors, fixture.manifest, fixture.signature, today()),
            fixture.publisher,
            fixture.fingerprint,
        );
    }
}

#[test]
fn real_cleartext_signatures_verify_and_return_only_signed_text() {
    let anchors = TrustAnchors::pinned();
    let (verification, text) = verify_openpgp_cleartext(anchors, FEDORA_44, today());
    expect_verified(
        verification,
        "Fedora 44",
        "36F612DCF27F7D1A48A835E4DBFCF71C6D9F90A6",
    );
    let text = text.expect("authenticated text");
    assert!(text.contains("1620295f6a00c27c3208f0c00b8ece4eab1ec69b9002152d97488bf26a426ddf"));
    assert!(!text.contains("BEGIN PGP"));

    let (verification, text) = verify_openpgp_cleartext(anchors, ALMA_10, today());
    expect_verified(
        verification,
        "AlmaLinux 10",
        "EE6DB7B98F5BF5EDD9DA0DE5DEE5C11CC2A1E572",
    );
    assert!(text.is_some());
}

#[test]
fn tampered_manifests_are_rejected_not_downgraded() {
    let anchors = TrustAnchors::pinned();
    for fixture in [&UBUNTU, &DEBIAN, &MINT, &KALI] {
        let verification = verify_openpgp_detached(
            anchors,
            &tampered(fixture.manifest),
            fixture.signature,
            today(),
        );
        assert!(
            matches!(verification, Verification::Rejected(_)),
            "{}: {verification:?}",
            fixture.publisher
        );
    }
}

#[test]
fn tampered_cleartext_is_rejected() {
    let anchors = TrustAnchors::pinned();
    let forged = FEDORA_44.replace("1620295f6a00", "0000000000000");
    assert_ne!(forged, FEDORA_44);
    let (verification, text) = verify_openpgp_cleartext(anchors, &forged, today());
    assert!(
        matches!(verification, Verification::Rejected(_)),
        "{verification:?}"
    );
    assert!(text.is_none());
}

#[test]
fn text_outside_the_signed_block_is_never_authenticated() {
    let anchors = TrustAnchors::pinned();
    // Unsigned text smuggled before or after the signed block makes the
    // document unreadable; nothing is ever reported as verified.
    let trailing = format!("{FEDORA_44}\nSHA256 (Evil.iso) = {}\n", "b".repeat(64));
    let leading = format!("SHA256 (Evil.iso) = {}\n{FEDORA_44}", "a".repeat(64));
    for document in [trailing, leading] {
        let (verification, text) = verify_openpgp_cleartext(anchors, &document, today());
        assert!(
            matches!(verification, Verification::Unverified(_)),
            "{verification:?}"
        );
        assert!(text.is_none());
    }
}

#[test]
fn a_signature_from_an_unpinned_key_proves_nothing() {
    // Ubuntu's signature, checked by a trust set that only pins Debian.
    let mut anchors = TrustAnchors::empty();
    let debian = pinned::OPENPGP_KEYS
        .iter()
        .find(|key| key.id == "debian-cd-2011")
        .expect("debian key");
    anchors
        .add_openpgp(debian.publisher, debian.fingerprint, debian.armored)
        .expect("pin debian");
    let verification =
        verify_openpgp_detached(&anchors, UBUNTU.manifest, UBUNTU.signature, today());
    match verification {
        Verification::Unverified(reason) => assert!(reason.contains("does not pin")),
        other => panic!("expected unverified, got {other:?}"),
    }
    let empty = TrustAnchors::empty();
    assert!(matches!(
        verify_openpgp_detached(&empty, UBUNTU.manifest, UBUNTU.signature, today()),
        Verification::Unverified(_)
    ));
}

#[test]
fn an_expired_pinned_key_degrades_instead_of_verifying() {
    let anchors = TrustAnchors::pinned();
    expect_verified(
        verify_openpgp_detached(anchors, KALI.manifest, KALI.signature, today()),
        KALI.publisher,
        KALI.fingerprint,
    );
    match verify_openpgp_detached(anchors, KALI.manifest, KALI.signature, after_kali_expiry()) {
        Verification::Unverified(reason) => assert!(reason.contains("expired"), "{reason}"),
        other => panic!("expected unverified, got {other:?}"),
    }
    // Keys without an expiry keep verifying.
    expect_verified(
        verify_openpgp_detached(
            anchors,
            UBUNTU.manifest,
            UBUNTU.signature,
            after_kali_expiry(),
        ),
        UBUNTU.publisher,
        UBUNTU.fingerprint,
    );
}

#[test]
fn a_revoked_pinned_key_never_verifies() {
    let mut anchors = TrustAnchors::empty();
    let ubuntu = &pinned::OPENPGP_KEYS[0];
    anchors
        .add_openpgp(ubuntu.publisher, ubuntu.fingerprint, ubuntu.armored)
        .expect("pin ubuntu");
    anchors.openpgp[0].revoked = true;
    match verify_openpgp_detached(&anchors, UBUNTU.manifest, UBUNTU.signature, today()) {
        Verification::Unverified(reason) => assert!(reason.contains("revoked"), "{reason}"),
        other => panic!("expected unverified, got {other:?}"),
    }
}

#[test]
fn unreadable_signature_files_are_not_signatures() {
    let anchors = TrustAnchors::pinned();
    for body in [
        &b""[..],
        b"<html><body>404 Not Found</body></html>",
        b"-----BEGIN PGP SIGNATURE-----\n\nnot base64 at all\n-----END PGP SIGNATURE-----\n",
        &UBUNTU.signature[..40],
    ] {
        let verification = verify_openpgp_detached(anchors, UBUNTU.manifest, body, today());
        assert!(
            matches!(verification, Verification::Unverified(_)),
            "{verification:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Generated keys: formats, tampering, wrong keys
// ---------------------------------------------------------------------------

fn generate(user: &str) -> SignedSecretKey {
    let params = SecretKeyParamsBuilder::default()
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(true)
        .primary_user_id(user.into())
        .build()
        .expect("key parameters");
    params.generate(thread_rng()).expect("generate key")
}

fn public_armor(key: &SignedSecretKey) -> String {
    SignedPublicKey::from(key.clone())
        .to_armored_string(ArmorOptions::default())
        .expect("armor public key")
}

fn pin(key: &SignedSecretKey, publisher: &str) -> TrustAnchors {
    let mut anchors = TrustAnchors::empty();
    anchors
        .add_openpgp(
            publisher,
            &format!("{:X}", key.fingerprint()),
            &public_armor(key),
        )
        .expect("pin generated key");
    anchors
}

const MANIFEST: &str = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa *demo-1.0.iso\n\
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb *demo-2.0.iso\n";

fn detach(key: &SignedSecretKey, data: &[u8]) -> DetachedSignature {
    DetachedSignature::sign_binary_data(
        thread_rng(),
        &key.primary_key,
        &Password::empty(),
        HashAlgorithm::Sha256,
        data,
    )
    .expect("sign")
}

#[test]
fn generated_binary_and_armored_detached_signatures_verify() {
    let key = generate("Demo Releases <releases@example.test>");
    let anchors = pin(&key, "Demo");
    let signature = detach(&key, MANIFEST.as_bytes());
    let binary = pgp::ser::Serialize::to_bytes(&signature).expect("binary signature");
    let armored = signature
        .to_armored_bytes(ArmorOptions::default())
        .expect("armored signature");
    let fingerprint = format!("{:X}", key.fingerprint());

    for bytes in [binary, armored] {
        expect_verified(
            verify_openpgp_detached(&anchors, MANIFEST.as_bytes(), &bytes, today()),
            "Demo",
            &fingerprint,
        );
    }
}

fn detach_with(key: &SignedSecretKey, hash: HashAlgorithm, data: &[u8]) -> Vec<u8> {
    let signature = DetachedSignature::sign_binary_data(
        thread_rng(),
        &key.primary_key,
        &Password::empty(),
        hash,
        data,
    )
    .expect("sign");
    pgp::ser::Serialize::to_bytes(&signature).expect("signature bytes")
}

#[test]
fn only_strong_hash_algorithms_verify() {
    // RSA can sign with every hash, unlike Ed25519 (which refuses weak ones).
    let params = SecretKeyParamsBuilder::default()
        .key_type(KeyType::Rsa(2048))
        .can_certify(true)
        .can_sign(true)
        .primary_user_id("Demo <demo@example.test>".into())
        .build()
        .expect("key parameters");
    let key = params.generate(thread_rng()).expect("generate RSA key");
    let anchors = pin(&key, "Demo");
    let fingerprint = format!("{:X}", key.fingerprint());
    for hash in [
        HashAlgorithm::Sha224,
        HashAlgorithm::Sha256,
        HashAlgorithm::Sha384,
        HashAlgorithm::Sha512,
        HashAlgorithm::Sha3_256,
        HashAlgorithm::Sha3_512,
    ] {
        let bytes = detach_with(&key, hash, MANIFEST.as_bytes());
        expect_verified(
            verify_openpgp_detached(&anchors, MANIFEST.as_bytes(), &bytes, today()),
            "Demo",
            &fingerprint,
        );
    }
    for hash in [
        HashAlgorithm::Md5,
        HashAlgorithm::Sha1,
        HashAlgorithm::Ripemd160,
    ] {
        let bytes = detach_with(&key, hash, MANIFEST.as_bytes());
        match verify_openpgp_detached(&anchors, MANIFEST.as_bytes(), &bytes, today()) {
            Verification::Unverified(reason) => {
                assert!(reason.contains("weak hash"), "{hash}: {reason}");
            }
            other => panic!("{hash} must not verify, got {other:?}"),
        }
    }
}

#[test]
fn generated_tampered_manifest_is_rejected() {
    let key = generate("Demo <demo@example.test>");
    let anchors = pin(&key, "Demo");
    let signature =
        pgp::ser::Serialize::to_bytes(&detach(&key, MANIFEST.as_bytes())).expect("signature");
    let altered = MANIFEST.replace("aaaa", "cccc");
    let verification = verify_openpgp_detached(&anchors, altered.as_bytes(), &signature, today());
    assert!(
        matches!(verification, Verification::Rejected(_)),
        "{verification:?}"
    );
}

#[test]
fn generated_signature_by_another_key_is_unverified_not_verified() {
    let pinned_key = generate("Pinned <pinned@example.test>");
    let attacker = generate("Attacker <attacker@example.test>");
    let anchors = pin(&pinned_key, "Pinned");
    let signature =
        pgp::ser::Serialize::to_bytes(&detach(&attacker, MANIFEST.as_bytes())).expect("signature");
    match verify_openpgp_detached(&anchors, MANIFEST.as_bytes(), &signature, today()) {
        Verification::Unverified(reason) => assert!(reason.contains("does not pin")),
        other => panic!("expected unverified, got {other:?}"),
    }
}

#[test]
fn generated_cleartext_message_round_trips_and_detects_tampering() {
    let key = generate("Demo <demo@example.test>");
    let anchors = pin(&key, "Demo");
    let message =
        CleartextSignedMessage::sign(thread_rng(), MANIFEST, &key.primary_key, &Password::empty())
            .expect("sign cleartext");
    let armored = message
        .to_armored_string(ArmorOptions::default())
        .expect("armor cleartext");

    let (verification, text) = verify_openpgp_cleartext(&anchors, &armored, today());
    assert!(
        matches!(verification, Verification::Verified(_)),
        "{verification:?}"
    );
    assert!(text.expect("text").contains("demo-2.0.iso"));

    let forged = armored.replace("bbbb", "dddd");
    let (verification, text) = verify_openpgp_cleartext(&anchors, &forged, today());
    assert!(
        matches!(verification, Verification::Rejected(_)),
        "{verification:?}"
    );
    assert!(text.is_none());
}

#[test]
fn cleartext_without_a_signature_block_is_unverified() {
    let anchors = pin(&generate("Demo <demo@example.test>"), "Demo");
    let (verification, _) = verify_openpgp_cleartext(&anchors, MANIFEST, today());
    assert!(matches!(verification, Verification::Unverified(_)));
}

// ---------------------------------------------------------------------------
// Minisign
// ---------------------------------------------------------------------------

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0_u32, |acc, (index, byte)| {
            acc | (u32::from(*byte) << (16 - 8 * index))
        });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[((value >> (18 - 6 * index)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

struct MinisignKey {
    secret: SigningKey,
    key_id: [u8; 8],
}

impl MinisignKey {
    fn new(seed: u8, key_id: [u8; 8]) -> Self {
        Self {
            secret: SigningKey::from_bytes(&[seed; 32]),
            key_id,
        }
    }

    fn public_base64(&self) -> String {
        let mut bytes = b"Ed".to_vec();
        bytes.extend_from_slice(&self.key_id);
        bytes.extend_from_slice(self.secret.verifying_key().as_bytes());
        base64(&bytes)
    }

    /// A minisign signature file in prehashed (`ED`) or legacy (`Ed`) mode.
    fn sign(&self, data: &[u8], prehashed: bool) -> String {
        use blake2::{Blake2b512, Digest};
        let signed = if prehashed {
            Blake2b512::digest(data).to_vec()
        } else {
            data.to_vec()
        };
        let signature = self.secret.sign(&signed).to_bytes();
        let mut line = if prehashed {
            b"ED".to_vec()
        } else {
            b"Ed".to_vec()
        };
        line.extend_from_slice(&self.key_id);
        line.extend_from_slice(&signature);
        let comment = "timestamp:1 file:demo";
        let mut global = signature.to_vec();
        global.extend_from_slice(comment.as_bytes());
        let global = self.secret.sign(&global).to_bytes();
        format!(
            "untrusted comment: signature from minisign secret key\n{}\ntrusted comment: {comment}\n{}\n",
            base64(&line),
            base64(&global)
        )
    }
}

fn minisign_anchors(key: &MinisignKey) -> TrustAnchors {
    let mut anchors = TrustAnchors::empty();
    anchors
        .add_minisign("Demo Minisign", &key.public_base64())
        .expect("pin minisign key");
    anchors
}

#[test]
fn minisign_prehashed_signature_verifies() {
    let key = MinisignKey::new(7, [1, 2, 3, 4, 5, 6, 7, 8]);
    let anchors = minisign_anchors(&key);
    let signature = key.sign(MANIFEST.as_bytes(), true);
    match verify_minisign(&anchors, MANIFEST.as_bytes(), &signature) {
        Verification::Verified(signer) => {
            assert_eq!(signer.protocol, SignatureProtocol::Minisign);
            assert_eq!(signer.publisher, "Demo Minisign");
            assert_eq!(signer.fingerprint, key.public_base64());
        }
        other => panic!("expected verified, got {other:?}"),
    }
}

#[test]
fn minisign_tampered_manifest_is_rejected() {
    let key = MinisignKey::new(7, [1, 2, 3, 4, 5, 6, 7, 8]);
    let anchors = minisign_anchors(&key);
    let signature = key.sign(MANIFEST.as_bytes(), true);
    let altered = MANIFEST.replace("aaaa", "cccc");
    assert!(matches!(
        verify_minisign(&anchors, altered.as_bytes(), &signature),
        Verification::Rejected(_)
    ));
}

#[test]
fn minisign_signature_by_an_unpinned_key_is_unverified() {
    let pinned = MinisignKey::new(7, [1, 2, 3, 4, 5, 6, 7, 8]);
    let other = MinisignKey::new(9, [8, 7, 6, 5, 4, 3, 2, 1]);
    let anchors = minisign_anchors(&pinned);
    let signature = other.sign(MANIFEST.as_bytes(), true);
    match verify_minisign(&anchors, MANIFEST.as_bytes(), &signature) {
        Verification::Unverified(reason) => assert!(reason.contains("does not pin")),
        other => panic!("expected unverified, got {other:?}"),
    }
}

#[test]
fn minisign_legacy_mode_is_not_accepted() {
    let key = MinisignKey::new(7, [1, 2, 3, 4, 5, 6, 7, 8]);
    let anchors = minisign_anchors(&key);
    let signature = key.sign(MANIFEST.as_bytes(), false);
    match verify_minisign(&anchors, MANIFEST.as_bytes(), &signature) {
        Verification::Unverified(reason) => assert!(reason.contains("legacy")),
        other => panic!("expected unverified, got {other:?}"),
    }
}

#[test]
fn minisign_garbage_is_unverified() {
    let key = MinisignKey::new(7, [1, 2, 3, 4, 5, 6, 7, 8]);
    let anchors = minisign_anchors(&key);
    assert!(matches!(
        verify_minisign(
            &anchors,
            MANIFEST.as_bytes(),
            "untrusted comment: x\nnope\n"
        ),
        Verification::Unverified(_)
    ));
}

// ---------------------------------------------------------------------------
// Manifest authentication (discovery of sibling signature files)
// ---------------------------------------------------------------------------

fn url(value: &str) -> Url {
    Url::parse(value).expect("test URL")
}

#[test]
fn signature_urls_are_siblings_of_the_manifest() {
    let manifest = url("https://releases.example.test/24.04/SHA256SUMS?mirror=1#frag");
    assert_eq!(
        signature_url(&manifest, ".gpg").expect("url").as_str(),
        "https://releases.example.test/24.04/SHA256SUMS.gpg"
    );
    assert!(signature_url(&url("https://example.test/dir/"), ".gpg").is_none());
}

#[test]
fn detached_manifest_signature_is_discovered_and_verified() {
    let requested = RefCell::new(Vec::new());
    let manifest_url = url("https://releases.ubuntu.com/24.04/SHA256SUMS");
    let result = authenticate_manifest(
        TrustAnchors::pinned(),
        &manifest_url,
        UBUNTU.manifest,
        today(),
        |candidate| {
            requested.borrow_mut().push(candidate.to_string());
            candidate
                .path()
                .ends_with(".gpg")
                .then(|| UBUNTU.signature.to_vec())
        },
    )
    .expect("authenticate");
    assert_eq!(
        requested.into_inner(),
        ["https://releases.ubuntu.com/24.04/SHA256SUMS.gpg"]
    );
    assert!(matches!(result.signature, ManifestSignature::Verified(_)));
    assert_eq!(result.text.as_bytes(), UBUNTU.manifest);
}

#[test]
fn debian_sign_suffix_is_found_after_gpg_is_absent() {
    let requested = RefCell::new(Vec::new());
    let result = authenticate_manifest(
        TrustAnchors::pinned(),
        &url("https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/SHA256SUMS"),
        DEBIAN.manifest,
        today(),
        |candidate| {
            requested.borrow_mut().push(candidate.path().to_owned());
            candidate
                .path()
                .ends_with(".sign")
                .then(|| DEBIAN.signature.to_vec())
        },
    )
    .expect("authenticate");
    assert!(matches!(result.signature, ManifestSignature::Verified(_)));
    assert_eq!(requested.borrow().len(), 2);
}

#[test]
fn missing_signature_degrades_to_unverified_with_a_truthful_reason() {
    let result = authenticate_manifest(
        TrustAnchors::pinned(),
        &url("https://mirror.example.test/iso/SHA256SUMS"),
        UBUNTU.manifest,
        today(),
        |_| None,
    )
    .expect("authenticate");
    match result.signature {
        ManifestSignature::Unverified(reason) => {
            assert!(reason.contains("does not publish a signature"), "{reason}");
        }
        other => panic!("expected unverified, got {other:?}"),
    }
    assert_eq!(result.text.as_bytes(), UBUNTU.manifest);
}

#[test]
fn soft_404_html_is_not_mistaken_for_a_signature() {
    let result = authenticate_manifest(
        TrustAnchors::pinned(),
        &url("https://mirror.example.test/iso/SHA256SUMS"),
        UBUNTU.manifest,
        today(),
        |_| Some(b"<html>Not Found</html>".to_vec()),
    )
    .expect("authenticate");
    assert!(matches!(result.signature, ManifestSignature::Unverified(_)));
}

#[test]
fn tampered_manifest_with_a_genuine_signature_refuses_the_download() {
    let error = authenticate_manifest(
        TrustAnchors::pinned(),
        &url("https://releases.ubuntu.com/24.04/SHA256SUMS"),
        &tampered(UBUNTU.manifest),
        today(),
        |_| Some(UBUNTU.signature.to_vec()),
    )
    .expect_err("tampering must refuse");
    let message = error.to_string();
    assert!(
        message.starts_with("download refused: signature verification failed"),
        "{message}"
    );
}

#[test]
fn clearsigned_manifest_is_verified_without_any_extra_request() {
    let result = authenticate_manifest(
        TrustAnchors::pinned(),
        &url("https://download.fedoraproject.org/pub/fedora/linux/releases/44/Workstation/x86_64/iso/Fedora-Workstation-44-1.7-x86_64-CHECKSUM"),
        FEDORA_44.as_bytes(),
        today(),
        |candidate| panic!("clearsigned manifests need no sibling request: {candidate}"),
    )
    .expect("authenticate");
    assert!(matches!(result.signature, ManifestSignature::Verified(_)));
    assert!(!result.text.contains("BEGIN PGP"));
}

#[test]
fn clearsigned_manifest_by_an_unknown_key_is_still_readable_but_unverified() {
    let key = generate("Unknown <unknown@example.test>");
    let message =
        CleartextSignedMessage::sign(thread_rng(), MANIFEST, &key.primary_key, &Password::empty())
            .expect("sign")
            .to_armored_string(ArmorOptions::default())
            .expect("armor");
    let result = authenticate_manifest(
        TrustAnchors::pinned(),
        &url("https://example.test/CHECKSUM"),
        message.as_bytes(),
        today(),
        |_| None,
    )
    .expect("authenticate");
    assert!(matches!(result.signature, ManifestSignature::Unverified(_)));
    assert!(result.text.contains("demo-1.0.iso"));
}

#[test]
fn signature_files_are_only_requested_for_protocols_with_a_pinned_key() {
    // No minisign key is pinned, so `.minisig` is never requested.
    let requested = RefCell::new(Vec::new());
    authenticate_manifest(
        TrustAnchors::pinned(),
        &url("https://example.test/SHA256SUMS"),
        MANIFEST.as_bytes(),
        today(),
        |candidate| {
            requested.borrow_mut().push(candidate.path().to_owned());
            None
        },
    )
    .expect("authenticate");
    assert!(
        requested
            .borrow()
            .iter()
            .all(|path| !path.ends_with(".minisig"))
    );
    // With no keys at all nothing is requested.
    let none = RefCell::new(0);
    authenticate_manifest(
        &TrustAnchors::empty(),
        &url("https://example.test/SHA256SUMS"),
        MANIFEST.as_bytes(),
        today(),
        |_| {
            *none.borrow_mut() += 1;
            None
        },
    )
    .expect("authenticate");
    assert_eq!(*none.borrow(), 0);
}

#[test]
fn minisign_manifest_signature_is_discovered_next_to_the_manifest() {
    let key = MinisignKey::new(3, [9, 9, 9, 9, 1, 1, 1, 1]);
    let anchors = minisign_anchors(&key);
    let signature = key.sign(MANIFEST.as_bytes(), true);
    let result = authenticate_manifest(
        &anchors,
        &url("https://example.test/dist/SHA256SUMS"),
        MANIFEST.as_bytes(),
        today(),
        |candidate| {
            candidate
                .path()
                .ends_with(".minisig")
                .then(|| signature.clone().into_bytes())
        },
    )
    .expect("authenticate");
    assert!(matches!(
        result.signature,
        ManifestSignature::Verified(SignerIdentity {
            protocol: SignatureProtocol::Minisign,
            ..
        })
    ));

    let error = authenticate_manifest(
        &anchors,
        &url("https://example.test/dist/SHA256SUMS"),
        MANIFEST.replace("aaaa", "eeee").as_bytes(),
        today(),
        |candidate| {
            candidate
                .path()
                .ends_with(".minisig")
                .then(|| signature.clone().into_bytes())
        },
    )
    .expect_err("tampering must refuse");
    assert!(error.to_string().contains("signature verification failed"));
}

#[test]
fn short_fingerprint_is_the_grouped_long_key_id() {
    let signer = SignerIdentity {
        publisher: "Ubuntu".into(),
        fingerprint: UBUNTU.fingerprint.into(),
        protocol: SignatureProtocol::OpenPgp,
    };
    assert_eq!(signer.short_fingerprint(), "D94A A3F0 EFE2 1092");
}
