//! Static metrics of benchmark files, computed from the source alone.
//!
//! - [`analyze_rust`]: a `.rs` file, every column of `rust_metrics`.
//! - [`analyze_viper`]: a `.vpr` file, every column of `viper_metrics`, using
//!   a Viper parser copied from ViperHelium.
//!
//! The metrics tables are keyed by the file's SHA-256 (`utils::hash_file`).

mod rust;
mod text;
mod viper;

use std::fmt;
use std::path::{Path, PathBuf};

pub use rust::{FnMetrics, RustMetrics};
pub use text::TextMetrics;
pub use viper::{Counts, ViperMetrics};

/// Analyzes the Rust file at `path`.
pub fn analyze_rust(path: impl AsRef<Path>) -> Result<RustMetrics, Error> {
    let path = path.as_ref();
    rust::analyze(&read(path)?).map_err(|e| Error::Parse(path.into(), e.to_string()))
}

/// Analyzes the Viper file at `path`.
pub fn analyze_viper(path: impl AsRef<Path>) -> Result<ViperMetrics, Error> {
    let path = path.as_ref();
    viper::analyze(&read(path)?).map_err(|e| Error::Parse(path.into(), e.to_string()))
}

fn read(path: &Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|e| Error::Io(path.into(), e))
}

/// Why a file could not be analyzed.
#[derive(Debug)]
pub enum Error {
    /// The file could not be read (or is not UTF-8).
    Io(PathBuf, std::io::Error),
    /// The file is not valid Rust / Viper.
    Parse(PathBuf, String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(path, e) => write!(f, "cannot read {}: {e}", path.display()),
            Error::Parse(path, e) => write!(f, "cannot parse {}: {e}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    fn files_with_extension(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files_with_extension(&path, ext, out);
            } else if path.extension().is_some_and(|e| e == ext) {
                out.push(path);
            }
        }
    }

    #[test]
    fn analyzes_files() {
        let dir = std::env::temp_dir().join(format!("file_analyzer_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rs = dir.join("x.rs");
        let vpr = dir.join("x.vpr");
        std::fs::write(&rs, "struct S;\nfn f() {}\n").unwrap();
        std::fs::write(&vpr, "field f: Int\nmethod m()\n").unwrap();

        let rust = analyze_rust(&rs).unwrap();
        assert_eq!((rust.fns, rust.structs), (1, 1));
        let viper = analyze_viper(&vpr).unwrap();
        assert_eq!((viper.methods, viper.fields), (1, 1));

        std::fs::write(&rs, "fn {").unwrap();
        assert!(matches!(analyze_rust(&rs), Err(Error::Parse(..))));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(analyze_viper(&vpr), Err(Error::Io(..))));
    }

    /// Every benchmark file must parse, or its metrics are lost.
    #[test]
    #[ignore = "reads every file under suites/ (run with --release)"]
    fn analyzes_every_suite_file() {
        let suites = Path::new(env!("CARGO_MANIFEST_DIR")).join("../suites");
        let mut failures = Vec::new();
        for (ext, analyze) in [
            ("rs", (|p: &Path| analyze_rust(p).map(drop)) as fn(&Path) -> Result<(), Error>),
            ("vpr", |p: &Path| analyze_viper(p).map(drop)),
        ] {
            let mut files = Vec::new();
            files_with_extension(&suites, ext, &mut files);
            assert!(!files.is_empty(), "no .{ext} files under {}", suites.display());
            failures.extend(files.iter().filter_map(|p| analyze(p).err().map(|e| e.to_string())));
        }
        assert!(failures.is_empty(), "{} files failed:\n{}", failures.len(), failures.join("\n"));
    }
}
