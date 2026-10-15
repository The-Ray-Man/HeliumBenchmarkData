//! Small helpers shared across the runner.

use std::fmt::Write;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of `bytes`, the key of the metrics tables.
pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Lowercase hex SHA-256 of the contents of the file at `path`.
pub fn hash_file(path: impl AsRef<Path>) -> std::io::Result<String> {
    Ok(sha256(&std::fs::read(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_lowercase_hex() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hashes_file_contents() {
        let path = std::env::temp_dir().join(format!("hash_file_test_{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(hash_file(&path).unwrap(), sha256(b"abc"));
        std::fs::remove_file(&path).unwrap();
        assert!(hash_file(&path).is_err());
    }
}
