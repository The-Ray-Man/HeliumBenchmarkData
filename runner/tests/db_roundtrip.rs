//! Needs the Postgres of compose.yaml: `cargo test -- --ignored`.

use db_generated::rustc_runs;
use db_generated::sea_orm_active_enums::RunStatus;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, Database, EntityTrait, TransactionTrait};

#[tokio::test]
#[ignore = "needs a running database"]
async fn insert_and_read_back() -> Result<(), sea_orm::DbErr> {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://helium:helium@localhost:5432/helium_bench".into());
    let db = Database::connect(url).await?;
    let txn = db.begin().await?;

    let row = rustc_runs::ActiveModel {
        run_id: Set("test_run".into()),
        started_at: Set(chrono::Utc::now().fixed_offset()),
        suite: Set("crates100".into()),
        test: Set("cfg-if".into()),
        vpr_sha256: Set("ab".repeat(32)),
        iteration: Set(0),
        status: Set(RunStatus::TimedOut),
        wall_s: Set(Some(1.25)),
        phases: Set(Some(serde_json::json!({"typeck": 0.25}))),
        ..Default::default()
    }
    .insert(&txn)
    .await?;

    let read = rustc_runs::Entity::find_by_id(row.id).one(&txn).await?.unwrap();
    assert_eq!(read, row);
    assert_eq!(read.family, None);

    // Rolled back, so the test leaves no rows behind.
    txn.rollback().await?;
    assert!(rustc_runs::Entity::find_by_id(row.id).one(&db).await?.is_none());
    Ok(())
}
