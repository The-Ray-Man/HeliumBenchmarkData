//! Static metrics of benchmark files, computed from the source alone.
//!
//! - [`rust`]: `.rs` files, every column of `rust_metrics`.
//! - [`viper`]: `.vpr` files, every column of `viper_metrics`, using a Viper
//!   parser copied from ViperHelium.
//! - [`text`]: line counting shared by both.
//!
//! The metrics tables are keyed by the file's SHA-256 (`utils::sha256`).

pub mod rust;
pub mod text;
pub mod viper;

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

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

    /// Every benchmark file must parse, or its metrics are lost.
    #[test]
    #[ignore = "reads every file under suites/ (run with --release)"]
    fn analyzes_every_suite_file() {
        let suites = Path::new(env!("CARGO_MANIFEST_DIR")).join("../suites");
        let mut failures = Vec::new();
        for (ext, analyze) in [
            ("rs", (|s: &str| super::rust::analyze(s).map(drop).map_err(|e| e.to_string())) as fn(&str) -> Result<(), String>),
            ("vpr", |s: &str| super::viper::analyze(s).map(drop).map_err(|e| e.to_string())),
        ] {
            let mut files = Vec::new();
            files_with_extension(&suites, ext, &mut files);
            assert!(!files.is_empty(), "no .{ext} files under {}", suites.display());
            for path in files {
                let source = std::fs::read_to_string(&path).unwrap();
                if let Err(e) = analyze(&source) {
                    failures.push(format!("{}: {e}", path.display()));
                }
            }
        }
        assert!(failures.is_empty(), "{} files failed:\n{}", failures.len(), failures.join("\n"));
    }
}
