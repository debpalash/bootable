//! The single description of how well a finished download was authenticated.
//!
//! Every user-visible integrity message (progress stage, ready line, ledger
//! row) is produced here, so the desktop and terminal interfaces cannot
//! disagree about what was verified.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ChecksumAlgorithm;
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

    /// The pinned key that vouched for the image, when there is one.
    pub fn signer(&self) -> Option<&SignerIdentity> {
        match self {
            Self::SignatureVerified { signer, .. } => Some(signer),
            _ => None,
        }
    }

    /// The shared one-line description of what was verified.
    pub fn label(&self) -> String {
        match self {
            Self::TransferChecked => {
                "HTTPS transfer and boot structure checked · publisher checksum unavailable".into()
            }
            Self::ChecksumVerified {
                signature_note: None,
                ..
            } => "Publisher checksum verified".into(),
            Self::ChecksumVerified {
                signature_note: Some(note),
                ..
            } => format!("Publisher checksum verified · signature not verified ({note})"),
            Self::SignatureVerified { algorithm, signer } => format!(
                "Signature verified · {} (key {}) · {algorithm} matches signed manifest",
                signer.publisher,
                signer.short_fingerprint()
            ),
        }
    }

    /// Ledger row text for a completed download.
    pub fn completion_message(&self) -> String {
        format!("{} · ready", self.label())
    }

    /// Progress text emitted once the verified file has been finalized.
    pub fn finalized_message(&self, destination: &Path) -> String {
        match self {
            Self::TransferChecked => format!(
                "Stage 4/5 · HTTPS transfer length verified · publisher checksum unavailable · finalized at {}",
                destination.display()
            ),
            Self::ChecksumVerified {
                algorithm,
                signature_note,
            } => match signature_note {
                None => format!(
                    "Stage 4/5 · Publisher {algorithm} verified · finalized at {}",
                    destination.display()
                ),
                Some(note) => format!(
                    "Stage 4/5 · Publisher {algorithm} verified · signature not verified ({note}) · finalized at {}",
                    destination.display()
                ),
            },
            Self::SignatureVerified { .. } => format!(
                "Stage 4/5 · {} · finalized at {}",
                self.label(),
                destination.display()
            ),
        }
    }

    /// Final progress text once the boot structure has been inspected.
    pub fn ready_message(&self, path: &Path) -> String {
        format!("Ready · {} · {}", self.label(), path.display())
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
