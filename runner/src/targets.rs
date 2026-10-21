//! Which Helium commits CI keeps runs for (`bench_targets`), and removing
//! the CI-owned runs of every other commit.
//!
//! `bench --ci` registers the commit it measures (`ci::start`): a main
//! commit for good, a PR head until [`close_pr`]. [`sync`] instead sets the
//! targets to exactly every main commit (from the start commit on) and the
//! current head of every open PR. Nothing else keeps CI-owned Helium runs.

use std::collections::HashSet;
use std::path::Path;

use log::info;
use sea_orm::ActiveValue::Set;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, IntoActiveModel,
    QueryFilter, QuerySelect, QueryTrait, TransactionTrait,
};

use crate::db::sea_orm_active_enums::BenchRunStatus;
use crate::db::{bench_runs, bench_targets, helium_runs};
use crate::utils::{git, git_output};

/// Helium's `main` branch on `origin`.
const MAIN: &str = "main";

/// The main commits from `start` on, oldest first, as `origin` has them
/// now: fetches `main` into `origin/main` in the checkout at `dir` (its
/// working tree is not touched), then follows first parents back to `start`.
/// Fails if `start` is not on main.
pub fn main_commits(dir: &Path, start: &str) -> std::io::Result<Vec<String>> {
    let remote = format!("origin/{MAIN}");
    git(dir, &["fetch", "--quiet", "origin", &format!("+refs/heads/{MAIN}:refs/remotes/{remote}")])?;
    if !git_output(dir, &["merge-base", "--is-ancestor", start, &remote])?.status.success() {
        return Err(std::io::Error::other(format!("start commit {start} is not on {remote}")));
    }
    let after = git(dir, &["rev-list", "--reverse", "--first-parent", &format!("{start}..{remote}"), "--"])?;
    let commits: Vec<String> = std::iter::once(start.to_string())
        .chain(after.lines().map(str::to_string))
        .collect();
    info!("{remote}: {} commits from {start} on", commits.len());
    Ok(commits)
}

/// The commits CI should have runs for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desired {
    /// Main commits from the start commit on, oldest first.
    pub main: Vec<String>,
    /// `(PR number, head commit)` of every open PR.
    pub prs: Vec<(i32, String)>,
}

impl Desired {
    /// One target per main commit and per PR: `(commit, pr_number, main_position)`.
    fn targets(&self) -> Vec<(&str, Option<i32>, Option<i32>)> {
        let main = self.main.iter().enumerate().map(|(i, c)| (c.as_str(), None, Some(i as i32)));
        let prs = self.prs.iter().map(|(n, c)| (c.as_str(), Some(*n), None));
        main.chain(prs).collect()
    }
}

/// Makes `bench_targets` exactly `desired`, removes the runs of commits no
/// longer targeted ([`gc`]), and returns the targeted commits without a
/// `COMPLETE` run: main commits first, oldest first, then PR heads.
pub async fn sync(con: &DatabaseConnection, desired: &Desired) -> Result<Vec<String>, DbErr> {
    let txn = con.begin().await?;
    let now = chrono::Local::now().fixed_offset();
    let wanted = desired.targets();
    let existing = bench_targets::Entity::find().all(&txn).await?;

    let mut removed = 0;
    for target in &existing {
        let key = (target.helium_commit.as_str(), target.pr_number);
        if !wanted.iter().any(|(c, pr, _)| (*c, *pr) == key) {
            bench_targets::Entity::delete_by_id(target.id).exec(&txn).await?;
            removed += 1;
        }
    }
    let (mut added, mut moved) = (0, 0);
    for &(commit, pr_number, main_position) in &wanted {
        match existing.iter().find(|t| t.helium_commit == commit && t.pr_number == pr_number) {
            Some(t) if t.main_position == main_position => {}
            Some(t) => {
                let mut row = t.clone().into_active_model();
                row.main_position = Set(main_position);
                row.updated_at = Set(now);
                row.update(&txn).await?;
                moved += 1;
            }
            None => {
                bench_targets::ActiveModel {
                    helium_commit: Set(commit.to_string()),
                    pr_number: Set(pr_number),
                    main_position: Set(main_position),
                    updated_at: Set(now),
                    ..Default::default()
                }
                .insert(&txn)
                .await?;
                added += 1;
            }
        }
    }
    info!(
        "targets: {} main commits, {} PRs ({added} added, {moved} moved, {removed} removed)",
        desired.main.len(),
        desired.prs.len()
    );
    gc(&txn).await?;
    txn.commit().await?;

    let complete: HashSet<String> = bench_runs::Entity::find()
        .select_only()
        .column(bench_runs::Column::HeliumCommit)
        .filter(bench_runs::Column::Status.eq(BenchRunStatus::Complete))
        .into_tuple()
        .all(con)
        .await?
        .into_iter()
        .collect();
    let mut missing = Vec::new();
    for (commit, _, _) in wanted {
        if !complete.contains(commit) && !missing.iter().any(|m| m == commit) {
            missing.push(commit.to_string());
        }
    }
    info!("{} targeted commits have no complete run", missing.len());
    Ok(missing)
}

/// Removes PR `pr_number`'s targets (all its heads) and the runs no longer
/// targeted ([`gc`]).
pub async fn close_pr(con: &DatabaseConnection, pr_number: i32) -> Result<(), DbErr> {
    let txn = con.begin().await?;
    let removed = bench_targets::Entity::delete_many()
        .filter(bench_targets::Column::PrNumber.eq(pr_number))
        .exec(&txn)
        .await?
        .rows_affected;
    info!("PR #{pr_number}: {removed} target(s) removed");
    gc(&txn).await?;
    txn.commit().await
}

/// Deletes every CI run of a commit without a target: its `helium_runs`
/// rows, then its `bench_runs` row. Runs still `RUNNING` are left alone, so
/// a benchmark in progress never loses its row (a later call removes it).
/// Silicon and rustc runs are kept: they don't depend on Helium's commit
/// and are shared by every run. Rows not owned by CI are never touched.
pub async fn gc<C: ConnectionTrait>(con: &C) -> Result<(), DbErr> {
    let targeted = bench_targets::Entity::find()
        .select_only()
        .column(bench_targets::Column::HeliumCommit)
        .into_query();
    let runs: Vec<(String, String)> = bench_runs::Entity::find()
        .select_only()
        .column(bench_runs::Column::RunId)
        .column(bench_runs::Column::HeliumCommit)
        .filter(bench_runs::Column::Status.ne(BenchRunStatus::Running))
        .filter(bench_runs::Column::HeliumCommit.not_in_subquery(targeted))
        .into_tuple()
        .all(con)
        .await?;
    if runs.is_empty() {
        return Ok(());
    }
    let run_ids: Vec<&str> = runs.iter().map(|(id, _)| id.as_str()).collect();
    let rows = helium_runs::Entity::delete_many()
        .filter(helium_runs::Column::RunId.is_in(run_ids.iter().copied()))
        .exec(con)
        .await?
        .rows_affected;
    bench_runs::Entity::delete_many()
        .filter(bench_runs::Column::RunId.is_in(run_ids.iter().copied()))
        .exec(con)
        .await?;
    for (run_id, commit) in &runs {
        info!("removed run {run_id} of {commit}");
    }
    info!("removed {} runs ({rows} Helium rows)", runs.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A commit in `dir` with no changes; returns its hash.
    fn commit(dir: &Path, message: &str) -> String {
        git(dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "--allow-empty", "-m", message]).unwrap();
        git(dir, &["rev-parse", "HEAD"]).unwrap()
    }

    #[test]
    fn lists_main_commits_from_origin() {
        let root = std::env::temp_dir().join(format!("main_commits_test_{}", std::process::id()));
        let (upstream, clone) = (root.join("upstream"), root.join("clone"));
        std::fs::create_dir_all(&upstream).unwrap();
        git(&upstream, &["init", "--quiet", "--initial-branch", MAIN]).unwrap();
        let c: Vec<String> = (1..=3).map(|i| commit(&upstream, &format!("c{i}"))).collect();
        git(&root, &["clone", "--quiet", "upstream", "clone"]).unwrap();

        assert_eq!(main_commits(&clone, &c[1]).unwrap(), [c[1].clone(), c[2].clone()]);
        assert_eq!(main_commits(&clone, &c[2]).unwrap(), [c[2].clone()]);

        // New commits on origin are fetched; the clone's checkout stays put.
        let c4 = commit(&upstream, "c4");
        assert_eq!(main_commits(&clone, &c[1]).unwrap(), [c[1].clone(), c[2].clone(), c4]);
        assert_eq!(git(&clone, &["rev-parse", "HEAD"]).unwrap(), c[2]);

        // A commit that is not on main.
        git(&upstream, &["checkout", "--quiet", "-b", "side", &c[0]]).unwrap();
        let side = commit(&upstream, "side");
        git(&clone, &["fetch", "--quiet", "origin", "side"]).unwrap();
        assert!(main_commits(&clone, &side).is_err());

        std::fs::remove_dir_all(&root).unwrap();
    }
}
