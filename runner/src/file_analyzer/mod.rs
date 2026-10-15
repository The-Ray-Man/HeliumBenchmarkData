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

/// A count as an `INTEGER` column. Saturates: no benchmark file comes near
/// `i32::MAX`, and a clamped value is still visibly an outlier.
fn db_int(n: u64) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
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

    #[test]
    fn converts_to_db_rows() {
        use crate::db::{rust_metrics, viper_metrics};
        use sea_orm::ActiveValue::{NotSet, Set};

        let rust: rust_metrics::ActiveModel = rust::analyze("fn f(x: i32) -> i32 { x + 1 }").unwrap().into();
        assert_eq!((rust.fns, rust.arith_ops), (Set(Some(1)), Set(Some(1))));
        assert_eq!(rust.paths, Set(Some(1.0)));
        // Left for the caller.
        assert_eq!((rust.rs_sha256, rust.vpr_sha256, rust.measured_at), (NotSet, NotSet, NotSet));

        let no_fns: rust_metrics::ActiveModel = rust::analyze("struct S;").unwrap().into();
        assert_eq!((no_fns.structs, no_fns.fn_loc, no_fns.paths), (Set(Some(1)), Set(None), Set(None)));

        let viper: viper_metrics::ActiveModel = viper::analyze("method m() { assert true }").unwrap().into();
        assert_eq!((viper.methods, viper.asserts), (Set(Some(1)), Set(Some(1))));
        assert_eq!(viper.vpr_sha256, NotSet);
    }

    /// Stores the metrics of a `.rs` file and its `.vpr` encoding, linked.
    #[tokio::test]
    #[ignore = "needs a running database"]
    async fn stores_linked_metrics() -> Result<(), sea_orm::DbErr> {
        use crate::db::{connection, queries, rust_metrics, viper_metrics};
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, TransactionTrait};

        let dir = std::env::temp_dir().join(format!("file_analyzer_db_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rs = dir.join("x.rs");
        let vpr = dir.join("x.vpr");
        std::fs::write(&rs, "struct S;
fn f() {}
").unwrap();
        std::fs::write(&vpr, "field f: Int
method m()
").unwrap();
        let (rs_hash, vpr_hash) = (crate::utils::hash_file(&rs).unwrap(), crate::utils::hash_file(&vpr).unwrap());
        let now = chrono::Utc::now().fixed_offset();

        let db = connection::connect().await?;
        let txn = db.begin().await?;
        // The Viper row first: rust_metrics.vpr_sha256 references it.
        viper_metrics::ActiveModel {
            vpr_sha256: Set(vpr_hash.clone()),
            measured_at: Set(now),
            ..analyze_viper(&vpr).unwrap().into()
        }
        .insert(&txn)
        .await?;
        rust_metrics::ActiveModel {
            rs_sha256: Set(rs_hash.clone()),
            vpr_sha256: Set(Some(vpr_hash.clone())),
            measured_at: Set(now),
            ..analyze_rust(&rs).unwrap().into()
        }
        .insert(&txn)
        .await?;

        let rust = queries::rust_metrics_of_file(&txn, &rs).await?.unwrap();
        assert_eq!((rust.fns, rust.structs, rust.vpr_sha256), (Some(1), Some(1), Some(vpr_hash)));
        let viper = queries::viper_metrics_of_file(&txn, &vpr).await?.unwrap();
        assert_eq!((viper.methods, viper.fields, viper.lines), (Some(1), Some(1), Some(2)));

        txn.rollback().await?;
        std::fs::remove_dir_all(&dir).unwrap();
        Ok(())
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
