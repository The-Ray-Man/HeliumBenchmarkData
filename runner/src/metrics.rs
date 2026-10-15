//! Storing the static metrics of a benchmark's files (see `file_analyzer`)
//! in `rust_metrics` / `viper_metrics`.

use std::path::Path;

use chrono::{DateTime, FixedOffset, Utc};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DbErr};

use crate::db::{rust_metrics, viper_metrics};
use crate::explorer::Benchmark;
use crate::file_analyzer;

/// Measures the benchmark's `.rs` and `.vpr` files (whichever it has) and
/// inserts one row per file, both with the same `measured_at`. Columns the
/// analyzer does not compute yet stay NULL.
pub async fn insert_metrics<C: ConnectionTrait>(db: &C, bench: &Benchmark) -> Result<(), DbErr> {
    let measured_at = Utc::now().fixed_offset();
    if let Some(path) = &bench.rust {
        insert_rust_metrics(db, path, measured_at).await?;
    }
    if let Some(path) = &bench.viper {
        insert_viper_metrics(db, path, measured_at).await?;
    }
    Ok(())
}

async fn insert_rust_metrics<C: ConnectionTrait>(
    db: &C,
    path: &Path,
    measured_at: DateTime<FixedOffset>,
) -> Result<(), DbErr> {
    let bytes = read(path)?;
    let source = text(path, &bytes)?;
    let m = file_analyzer::rust::analyze(source)
        .map_err(|e| DbErr::Custom(format!("cannot parse {}: {e}", path.display())))?;
    rust_metrics::ActiveModel {
        rs_sha256: Set(file_analyzer::sha256(&bytes)),
        measured_at: Set(measured_at),
        loc: Set(Some(int(m.text.loc)?)),
        fns: Set(Some(int(m.fns)?)),
        structs: Set(Some(int(m.structs)?)),
        enums: Set(Some(int(m.enums)?)),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(())
}

async fn insert_viper_metrics<C: ConnectionTrait>(
    db: &C,
    path: &Path,
    measured_at: DateTime<FixedOffset>,
) -> Result<(), DbErr> {
    let bytes = read(path)?;
    let m = file_analyzer::viper::analyze(text(path, &bytes)?);
    viper_metrics::ActiveModel {
        vpr_sha256: Set(file_analyzer::sha256(&bytes)),
        measured_at: Set(measured_at),
        loc: Set(Some(int(m.text.loc)?)),
        lines: Set(Some(int(m.text.lines)?)),
        bytes: Set(Some(i64::try_from(m.text.bytes).map_err(too_large)?)),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, DbErr> {
    std::fs::read(path).map_err(|e| DbErr::Custom(format!("cannot read {}: {e}", path.display())))
}

fn text<'a>(path: &Path, bytes: &'a [u8]) -> Result<&'a str, DbErr> {
    std::str::from_utf8(bytes)
        .map_err(|e| DbErr::Custom(format!("{} is not UTF-8: {e}", path.display())))
}

/// A count as an `INTEGER` column.
fn int(n: u64) -> Result<i32, DbErr> {
    i32::try_from(n).map_err(too_large)
}

fn too_large(e: std::num::TryFromIntError) -> DbErr {
    DbErr::Custom(format!("metric too large for its column: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::TransactionTrait;

    #[tokio::test]
    #[ignore = "needs a running database"]
    async fn inserts_metrics_of_both_files() -> Result<(), DbErr> {
        let dir = std::env::temp_dir().join(format!("metrics_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rs = dir.join("x.rs");
        let vpr = dir.join("x.vpr");
        std::fs::write(&rs, "// test\nstruct S;\nfn f() {}\n").unwrap();
        std::fs::write(&vpr, "field f: Int\n\nmethod m()\n{}\n").unwrap();
        let bench = Benchmark {
            suite: "s".into(),
            family: None,
            test: "x".into(),
            rust: Some(rs.clone()),
            viper: Some(vpr.clone()),
        };

        let db = crate::db::connection::connect().await?;
        let txn = db.begin().await?;
        insert_metrics(&txn, &bench).await?;

        let rs_hash = file_analyzer::sha256(&std::fs::read(&rs).unwrap());
        let rust = crate::db::queries::rust_metrics_by_sha256(&txn, &rs_hash).await?.unwrap();
        assert_eq!((rust.loc, rust.fns, rust.structs, rust.enums), (Some(2), Some(1), Some(1), Some(0)));
        assert_eq!(rust.stmts, None);

        let vpr_hash = file_analyzer::sha256(&std::fs::read(&vpr).unwrap());
        let viper = crate::db::queries::viper_metrics_by_sha256(&txn, &vpr_hash).await?.unwrap();
        assert_eq!((viper.loc, viper.lines), (Some(3), Some(4)));
        assert_eq!(viper.measured_at, rust.measured_at);

        txn.rollback().await?;
        std::fs::remove_dir_all(&dir).unwrap();
        Ok(())
    }
}
