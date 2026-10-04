//! The single description of how well a finished download was authenticated.
//!
//! Every user-visible integrity message (progress stage, ready line, ledger
//! row) is produced here, so the desktop and terminal interfaces cannot
//! disagree about what was verified.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ChecksumAlgorithm;
use crate::locale::Locale;
use crate::messages::Message;
use crate::signature::SignerIdentity;

/// Ordered from weakest to strongest evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntegrityState {
    /// No publisher checksum exists; only the HTTPS transfer length and the
    /// boot structure were checked.
    TransferChecked,
    /// The image matches a checksum published by the distributor. If the
    /// checksum came from a manifest, `signature_note` says why the manifest
    /// signature could not be used.
    ChecksumVerified {
        algorithm: ChecksumAlgorithm,
        signature_note: Option<String>,
        /// The checksum came from a publisher known to sign its manifests, but
        /// no usable signature was obtained (missing, unfetchable, unknown key,
        /// weak hash). This is a possible downgrade, not an ordinary unsigned
        /// publisher; adapters can refuse it with `require_signature`.
        #[serde(default)]
        signature_expected: bool,
    },
    /// The image matches a checksum from a manifest signed by a pinned key.
    SignatureVerified {
        algorithm: ChecksumAlgorithm,
        signer: SignerIdentity,
    },
}

impl IntegrityState {
    /// Strength of the evidence: 0 transfer only, 1 checksum, 2 signed checksum.
    pub fn rank(&self) -> u8 {
        match self {
            Self::TransferChecked => 0,
            Self::ChecksumVerified { .. } => 1,
            Self::SignatureVerified { .. } => 2,
        }
    }

    pub fn is_signature_verified(&self) -> bool {
        matches!(self, Self::SignatureVerified { .. })
    }

    /// True when a signature was expected from this publisher but could not be
    /// verified, so integrity fell back to the bare checksum.
    pub fn signature_expected_but_unverified(&self) -> bool {
        matches!(
            self,
            Self::ChecksumVerified {
                signature_expected: true,
                ..
            }
        )
    }

    /// The pinned key that vouched for the image, when there is one.
    pub fn signer(&self) -> Option<&SignerIdentity> {
        match self {
            Self::SignatureVerified { signer, .. } => Some(signer),
            _ => None,
        }
    }

    /// The shared one-line description of what was verified.
    pub fn label(&self) -> String {
        self.label_in(Locale::SOURCE)
    }

    /// [`IntegrityState::label`] in `locale`. The fixed phrases are translated;
    /// publisher names, fingerprints, and any `signature_note` are data and
    /// appear as produced.
    pub fn label_in(&self, locale: Locale) -> String {
        match self {
            Self::TransferChecked => Message::IntegrityTransferChecked.text(locale).into(),
            Self::ChecksumVerified {
                signature_note: None,
                ..
            } => Message::IntegrityChecksumVerified.text(locale).into(),
            Self::ChecksumVerified {
                signature_note: Some(note),
                ..
            } => Message::IntegrityChecksumUnsigned.format(locale, &[("note", note)]),
            Self::SignatureVerified { algorithm, signer } => Message::IntegritySignatureVerified
                .format(
                    locale,
                    &[
                        ("publisher", &signer.publisher),
                        ("fingerprint", &signer.short_fingerprint()),
                        ("algorithm", algorithm),
                    ],
                ),
        }
    }

    /// Ledger row text for a completed download.
    pub fn completion_message(&self) -> String {
        self.completion_message_in(Locale::SOURCE)
    }

    pub fn completion_message_in(&self, locale: Locale) -> String {
        Message::IntegrityCompletion.format(locale, &[("label", &self.label_in(locale))])
    }

    /// Progress text emitted once the verified file has been finalized.
    pub fn finalized_message(&self, destination: &Path) -> String {
        self.finalized_message_in(Locale::SOURCE, destination)
    }

    pub fn finalized_message_in(&self, locale: Locale, destination: &Path) -> String {
        let path = destination.display();
        match self {
            Self::TransferChecked => {
                Message::IntegrityFinalizedTransfer.format(locale, &[("path", &path)])
            }
            Self::ChecksumVerified {
                algorithm,
                signature_note,
                ..
            } => match signature_note {
                None => Message::IntegrityFinalizedChecksum
                    .format(locale, &[("algorithm", algorithm), ("path", &path)]),
                Some(note) => Message::IntegrityFinalizedChecksumUnsigned.format(
                    locale,
                    &[("algorithm", algorithm), ("note", note), ("path", &path)],
                ),
            },
            Self::SignatureVerified { .. } => Message::IntegrityFinalizedSignature.format(
                locale,
                &[("label", &self.label_in(locale)), ("path", &path)],
            ),
        }
    }

    /// Final progress text once the boot structure has been inspected.
    pub fn ready_message(&self, path: &Path) -> String {
        self.ready_message_in(Locale::SOURCE, path)
    }

    pub fn ready_message_in(&self, locale: Locale, path: &Path) -> String {
        Message::IntegrityReady.format(
            locale,
            &[("label", &self.label_in(locale)), ("path", &path.display())],
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::signature::SignatureProtocol;

    fn signer() -> SignerIdentity {
        SignerIdentity {
            publisher: "Ubuntu".into(),
            fingerprint: "843938DF228D22F7B3742BC0D94AA3F0EFE21092".into(),
            protocol: SignatureProtocol::OpenPgp,
        }
    }

    #[test]
    fn signature_outranks_checksum_outranks_transfer() {
        let transfer = IntegrityState::TransferChecked;
        let checksum = IntegrityState::ChecksumVerified {
            algorithm: ChecksumAlgorithm::Sha256,
            signature_note: None,
            signature_expected: false,
        };
        let signed = IntegrityState::SignatureVerified {
            algorithm: ChecksumAlgorithm::Sha256,
            signer: signer(),
        };
        assert!(transfer.rank() < checksum.rank());
        assert!(checksum.rank() < signed.rank());
        assert!(signed.is_signature_verified());
        assert!(!checksum.is_signature_verified());
        assert_eq!(
            signed.signer().map(|s| s.publisher.as_str()),
            Some("Ubuntu")
        );
        assert!(checksum.signer().is_none());
    }

    #[test]
    fn labels_never_claim_a_signature_without_one() {
        let checksum = IntegrityState::ChecksumVerified {
            algorithm: ChecksumAlgorithm::Sha256,
            signature_note: Some("the publisher does not publish a signature".into()),
            signature_expected: false,
        };
        let label = checksum.label();
        assert!(label.starts_with("Publisher checksum verified"));
        assert!(label.contains("signature not verified"));
        assert!(!label.contains("Signature verified"));
        assert!(
            !IntegrityState::TransferChecked
                .label()
                .contains("Signature verified")
        );
    }

    #[test]
    fn every_integrity_message_translates_with_all_data_intact() {
        let states = [
            IntegrityState::TransferChecked,
            IntegrityState::ChecksumVerified {
                algorithm: ChecksumAlgorithm::Sha256,
                signature_note: None,
                signature_expected: false,
            },
            IntegrityState::ChecksumVerified {
                algorithm: ChecksumAlgorithm::Sha512,
                signature_note: Some("NOTE-DATA".into()),
                signature_expected: true,
            },
            IntegrityState::SignatureVerified {
                algorithm: ChecksumAlgorithm::Sha256,
                signer: signer(),
            },
        ];
        let path = Path::new("/tmp/a.iso");
        for locale in Locale::ALL {
            for state in &states {
                for text in [
                    state.label_in(*locale),
                    state.completion_message_in(*locale),
                    state.finalized_message_in(*locale, path),
                    state.ready_message_in(*locale, path),
                ] {
                    assert!(
                        !text.contains('{') && !text.contains('}'),
                        "{locale}: {text}"
                    );
                }
                assert!(
                    state
                        .finalized_message_in(*locale, path)
                        .contains("/tmp/a.iso")
                );
                assert!(state.ready_message_in(*locale, path).contains("/tmp/a.iso"));
                if let IntegrityState::ChecksumVerified {
                    signature_note: Some(_),
                    ..
                } = state
                {
                    assert!(state.label_in(*locale).contains("NOTE-DATA"));
                }
                if state.is_signature_verified() {
                    assert!(state.label_in(*locale).contains("Ubuntu"));
                    assert!(state.label_in(*locale).contains("D94A A3F0 EFE2 1092"));
                }
            }
        }
        let spanish = states[3].label_in(Locale::Es);
        assert!(spanish.starts_with("Firma verificada"));
    }

    #[test]
    fn signed_label_names_the_key() {
        let signed = IntegrityState::SignatureVerified {
            algorithm: ChecksumAlgorithm::Sha256,
            signer: signer(),
        };
        assert_eq!(
            signed.label(),
            "Signature verified · Ubuntu (key D94A A3F0 EFE2 1092) · SHA-256 matches signed manifest"
        );
        assert!(signed.completion_message().ends_with(" · ready"));
        assert!(
            signed
                .ready_message(Path::new("/tmp/a.iso"))
                .starts_with("Ready · Signature verified")
        );
        assert!(
            signed
                .finalized_message(Path::new("/tmp/a.iso"))
                .contains("key D94A A3F0 EFE2 1092")
        );
    }
}
