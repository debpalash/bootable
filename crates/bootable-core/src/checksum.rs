use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use md5::Md5;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

use crate::error::{Error, Result, io_error};
use crate::operation::OperationControl;

const BUFFER_SIZE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChecksumAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

impl ChecksumAlgorithm {
    pub fn next(self) -> Self {
        match self {
            Self::Md5 => Self::Sha1,
            Self::Sha1 => Self::Sha256,
            Self::Sha256 => Self::Sha512,
            Self::Sha512 => Self::Md5,
        }
    }
}

impl fmt::Display for ChecksumAlgorithm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Md5 => "MD5",
            Self::Sha1 => "SHA-1",
            Self::Sha256 => "SHA-256",
            Self::Sha512 => "SHA-512",
        })
    }
}

impl FromStr for ChecksumAlgorithm {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().replace('-', "").as_str() {
            "md5" => Ok(Self::Md5),
            "sha1" => Ok(Self::Sha1),
            "sha256" => Ok(Self::Sha256),
            "sha512" => Ok(Self::Sha512),
            _ => Err(Error::UnsupportedImage(format!(
                "unknown checksum algorithm `{value}`; use md5, sha1, sha256, or sha512"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checksum {
    pub path: PathBuf,
    pub algorithm: ChecksumAlgorithm,
    pub hexadecimal: String,
}

pub(crate) fn compute(path: &Path, algorithm: ChecksumAlgorithm) -> Result<Checksum> {
    compute_controlled(path, algorithm, &OperationControl::new())
}

pub(crate) fn compute_controlled(
    path: &Path,
    algorithm: ChecksumAlgorithm,
    control: &OperationControl,
) -> Result<Checksum> {
    let file = File::open(path).map_err(|error| io_error(path, error))?;
    let mut reader = BufReader::with_capacity(BUFFER_SIZE, file);
    let hexadecimal = match algorithm {
        ChecksumAlgorithm::Md5 => digest::<Md5>(&mut reader, path, control)?,
        ChecksumAlgorithm::Sha1 => digest::<Sha1>(&mut reader, path, control)?,
        ChecksumAlgorithm::Sha256 => digest::<Sha256>(&mut reader, path, control)?,
        ChecksumAlgorithm::Sha512 => digest::<Sha512>(&mut reader, path, control)?,
    };
    Ok(Checksum {
        path: path.to_path_buf(),
        algorithm,
        hexadecimal,
    })
}

fn digest<D: Digest + Default>(
    reader: &mut impl Read,
    path: &Path,
    control: &OperationControl,
) -> Result<String> {
    let mut digest = D::default();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        control.checkpoint()?;
        let count = reader
            .read(&mut buffer)
            .map_err(|error| io_error(path, error))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let bytes = digest.finalize();
    let mut hexadecimal = String::with_capacity(bytes.len() * 2);
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        hexadecimal.push(DIGITS[(byte >> 4) as usize] as char);
        hexadecimal.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    Ok(hexadecimal)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn computes_all_supported_digests() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("abc.iso");
        fs::write(&path, b"abc").expect("write fixture");

        let expected = [
            (ChecksumAlgorithm::Md5, "900150983cd24fb0d6963f7d28e17f72"),
            (
                ChecksumAlgorithm::Sha1,
                "a9993e364706816aba3e25717850c26c9cd0d89d",
            ),
            (
                ChecksumAlgorithm::Sha256,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                ChecksumAlgorithm::Sha512,
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
        ];

        for (algorithm, expected) in expected {
            let checksum = compute(&path, algorithm).expect("checksum");
            assert_eq!(checksum.hexadecimal, expected);
        }
    }
}
