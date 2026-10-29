//! Writes that insert a row or overwrite the existing one.

use sea_orm::ActiveValue::Set;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ConnectionTrait, DbErr, EntityTrait};

use super::entities::{benchmarks, viper_sources};

/// Records which files a benchmark (`suite`, `family`, `test`) currently
/// consists of, by hash. Overwrites the benchmark's row if it has one. The
/// metrics rows of both hashes must exist already (foreign keys).
pub async fn upsert_benchmark<C: ConnectionTrait>(
    db: &C,
    suite: &str,
    family: Option<&str>,
    test: &str,
    vpr_sha256: Option<String>,
    rs_sha256: Option<String>,
) -> Result<(), DbErr> {
    let row = benchmarks::ActiveModel {
        suite: Set(suite.to_owned()),
        family: Set(family.map(str::to_owned)),
        test: Set(test.to_owned()),
        vpr_sha256: Set(vpr_sha256),
        rs_sha256: Set(rs_sha256),
        recorded_at: Set(chrono::Utc::now().fixed_offset()),
        ..Default::default()
    };
    benchmarks::Entity::insert(row)
        .on_conflict(
            // The UNIQUE NULLS NOT DISTINCT constraint: flat suites (NULL
            // family) conflict too.
            OnConflict::columns([
                benchmarks::Column::Suite,
                benchmarks::Column::Family,
                benchmarks::Column::Test,
            ])
            .update_columns([
                benchmarks::Column::VprSha256,
                benchmarks::Column::RsSha256,
                benchmarks::Column::RecordedAt,
            ])
            .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

/// Stores the source of the Viper file `vpr_sha256` unless it is stored
/// already; `read` is only called then. Returns whether it stored it. The
/// metrics row must exist already (foreign key).
pub async fn insert_viper_source<C: ConnectionTrait, E: From<DbErr>>(
    db: &C,
    vpr_sha256: &str,
    read: impl FnOnce() -> Result<String, E>,
) -> Result<bool, E> {
    if viper_sources::Entity::find_by_id(vpr_sha256).one(db).await?.is_some() {
        return Ok(false);
    }
    let row = viper_sources::ActiveModel { vpr_sha256: Set(vpr_sha256.to_owned()), source: Set(read()?) };
    // A concurrent writer may have stored it in between.
    let inserted = viper_sources::Entity::insert(row)
        .on_conflict(OnConflict::column(viper_sources::Column::VprSha256).do_nothing().to_owned())
        .exec_without_returning(db)
        .await?;
    Ok(inserted > 0)
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, TransactionTrait};

    use super::*;
    use crate::db::{connection, viper_metrics};

    #[tokio::test]
    #[ignore = "needs a running database"]
    async fn upsert_overwrites_flat_suite_benchmark() -> Result<(), DbErr> {
        let db = connection::connect().await?;
        let txn = db.begin().await?;
        let (old, new) = ("01".repeat(32), "02".repeat(32));
        for hash in [&old, &new] {
            viper_metrics::ActiveModel {
                vpr_sha256: Set(hash.clone()),
                measured_at: Set(chrono::Utc::now().fixed_offset()),
                ..Default::default()
            }
            .insert(&txn)
            .await?;
        }

        // Flat suite (no family): the NULL family must still conflict.
        upsert_benchmark(&txn, "test_suite", None, "t", Some(old), None).await?;
        upsert_benchmark(&txn, "test_suite", None, "t", Some(new.clone()), None).await?;

        let rows = benchmarks::Entity::find()
            .filter(benchmarks::Column::Suite.eq("test_suite"))
            .all(&txn)
            .await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].vpr_sha256, Some(new));
        txn.rollback().await?;
        Ok(())
    }
}
