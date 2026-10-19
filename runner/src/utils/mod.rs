//! Small helpers shared across the runner.

use std::fmt::Write;
use std::path::Path;
use std::process::{Command, Output};

use log::debug;
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

/// Runs `git <args>` in `dir`, whatever its exit status.
pub fn git_output(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    debug!("git {} in {}", args.join(" "), dir.display());
    Command::new("git").args(args).current_dir(dir).output()
}

/// Runs `git <args>` in `dir` and returns its trimmed stdout; a non-zero exit
/// is an error with git's stderr.
pub fn git(dir: &Path, args: &[&str]) -> std::io::Result<String> {
    let out = git_output(dir, args)?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "git {} in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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
