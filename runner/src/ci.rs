//! CI mode: the run is recorded in `bench_runs`, which makes its rows
//! CI-owned (see db/migrations/002_ci_runs.sql).
//!
//! The row is inserted as `RUNNING` before anything is measured and ends as
//! - `COMPLETE` when every benchmark was gone through, whatever the
//!   verification results (failures and timeouts are checked by hand);
//! - `FAILED` when the runner itself panicked midway ([`mark_failed`]);
//! - `CANCELLED` on Ctrl-C / SIGTERM, which GitHub Actions sends when a job
//!   is cancelled ([`watch_cancel`]).

use std::sync::OnceLock;

use log::{error, info, warn};
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};

use crate::db::bench_runs;
use crate::db::sea_orm_active_enums::BenchRunStatus;
use crate::executor::helium::BenchRun;

/// The run id once its `RUNNING` row is in the database, so a crash or a
/// cancel only ever updates the row this process inserted.
static STARTED: OnceLock<String> = OnceLock::new();

/// Inserts the `RUNNING` row of `run`. Fails if the run id is taken.
pub async fn start<C: ConnectionTrait>(
    con: &C,
    run: &BenchRun,
    helium_commit: &str,
    github_run_url: Option<&str>,
) -> Result<(), DbErr> {
    bench_runs::ActiveModel {
        run_id: Set(run.run_id.clone()),
        helium_commit: Set(helium_commit.to_string()),
        host: Set(run.host.clone()),
        github_run_url: Set(github_run_url.map(str::to_string)),
        status: Set(BenchRunStatus::Running),
        started_at: Set(run.started_at),
        finished_at: Set(None),
    }
    .insert(con)
    .await?;
    STARTED.set(run.run_id.clone()).expect("one CI run per process");
    info!("CI run {} started for Helium {helium_commit}", run.run_id);
    Ok(())
}

/// Sets the started run to `status`, unless it already left `RUNNING`.
pub async fn finish<C: ConnectionTrait>(con: &C, status: BenchRunStatus) -> Result<(), DbErr> {
    let Some(run_id) = STARTED.get() else {
        return Ok(());
    };
    bench_runs::Entity::update_many()
        .col_expr(bench_runs::Column::Status, status.into())
        .col_expr(bench_runs::Column::FinishedAt, chrono::Local::now().fixed_offset().into())
        .filter(bench_runs::Column::RunId.eq(run_id))
        .filter(bench_runs::Column::Status.eq(BenchRunStatus::Running))
        .exec(con)
        .await?;
    info!("CI run {run_id}: {status:?}");
    Ok(())
}

/// After the runner thread panicked: marks the started run `FAILED`, on a
/// connection of its own (the runner's runtime is gone).
pub fn mark_failed() {
    if STARTED.get().is_none() {
        return;
    }
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| DbErr::Custom(e.to_string()))
        .and_then(|rt| {
            rt.block_on(async {
                let con = crate::db::connection::connect().await?;
                finish(&con, BenchRunStatus::Failed).await
            })
        });
    if let Err(err) = result {
        error!("could not mark the CI run as FAILED: {err}");
    }
}

/// Marks the started run `CANCELLED` and exits on Ctrl-C (and SIGTERM on
/// Unix). Runs on the runtime's worker threads, so it also fires while the
/// runner thread waits on a benchmark process.
pub fn watch_cancel(con: DatabaseConnection) {
    tokio::spawn(async move {
        cancelled().await;
        warn!("cancelled");
        if let Err(err) = finish(&con, BenchRunStatus::Cancelled).await {
            error!("could not mark the CI run as CANCELLED: {err}");
        }
        std::process::exit(130);
    });
}

async fn cancelled() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("a SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await.expect("a Ctrl-C handler");
}

/// A full 40-character commit hash, as `bench_runs.helium_commit` and
/// `helium_runs.backend_version` store it.
pub fn parse_commit(s: &str) -> Result<String, String> {
    if s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(s.to_ascii_lowercase())
    } else {
        Err("expected a full 40-character commit hash".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_commits_only() {
        let full = "B81D12363bed9fae54810183b94e353736205456";
        assert_eq!(parse_commit(full).unwrap(), full.to_ascii_lowercase());
        assert!(parse_commit("b81d123").is_err());
        assert!(parse_commit(&"g".repeat(40)).is_err());
    }
}
