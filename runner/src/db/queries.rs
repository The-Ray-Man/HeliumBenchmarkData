//! Lookups of file metrics. Each returns the file's row, or `None` if the file
//! was never measured; [`benchmark_metrics_status`] says what a benchmark's
//! rows still need.

use std::path::Path;

use sea_orm::{ConnectionTrait, DbErr, EntityTrait};

use super::entities::{rust_metrics, viper_metrics};
use crate::explorer::Benchmark;
use crate::utils;

/// Viper metrics of the file with SHA-256 `vpr_sha256` (lowercase hex).
pub async fn viper_metrics_by_sha256<C: ConnectionTrait>(
    db: &C,
    vpr_sha256: &str,
) -> Result<Option<viper_metrics::Model>, DbErr> {
    viper_metrics::Entity::find_by_id(vpr_sha256).one(db).await
}

/// Rust metrics of the file with SHA-256 `rs_sha256` (lowercase hex).
pub async fn rust_metrics_by_sha256<C: ConnectionTrait>(
    db: &C,
    rs_sha256: &str,
) -> Result<Option<rust_metrics::Model>, DbErr> {
    rust_metrics::Entity::find_by_id(rs_sha256).one(db).await
}

/// Metrics of the `.vpr` file at `path`, looked up by its content hash.
pub async fn viper_metrics_of_file<C: ConnectionTrait>(
    db: &C,
    path: impl AsRef<Path>,
) -> Result<Option<viper_metrics::Model>, DbErr> {
    viper_metrics_by_sha256(db, &hash_file(path.as_ref())?).await
}

/// Metrics of the `.rs` file at `path`, looked up by its content hash.
pub async fn rust_metrics_of_file<C: ConnectionTrait>(
    db: &C,
    path: impl AsRef<Path>,
) -> Result<Option<rust_metrics::Model>, DbErr> {
    rust_metrics_by_sha256(db, &hash_file(path.as_ref())?).await
}

fn hash_file(path: &Path) -> Result<String, DbErr> {
    utils::hash_file(path).map_err(|e| DbErr::Custom(format!("cannot read {}: {e}", path.display())))
}

/// What a file's metrics row needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetricsAction {
    /// The file has no row yet.
    Insert,
    /// The row exists, but these columns are NULL or wrong.
    Update(Vec<&'static str>),
    /// The row is complete and, for a Rust file, linked to the right Viper file.
    UpToDate,
}

/// A benchmark file's hash and what its metrics row needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMetricsStatus {
    pub sha256: String,
    pub action: MetricsAction,
}

/// What a benchmark's metrics rows need; `None` for a file it does not have.
/// Apply `viper` before `rust`: `rust_metrics.vpr_sha256` references the
/// Viper row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchmarkMetricsStatus {
    pub viper: Option<FileMetricsStatus>,
    pub rust: Option<FileMetricsStatus>,
}

/// Looks up the metrics rows of `bench`'s `.vpr` and `.rs` files (by content
/// hash) and says, for each, whether it must be inserted, updated or is up to
/// date. A Rust row must point to the hash of the benchmark's Viper file, or
/// to nothing if it has none.
pub async fn benchmark_metrics_status<C: ConnectionTrait>(
    db: &C,
    bench: &Benchmark,
) -> Result<BenchmarkMetricsStatus, DbErr> {
    let viper = match &bench.viper {
        Some(path) => {
            let sha256 = hash_file(path)?;
            let action = match viper_metrics_by_sha256(db, &sha256).await? {
                None => MetricsAction::Insert,
                Some(row) => action_for(viper_problems(row)),
            };
            Some(FileMetricsStatus { sha256, action })
        }
        None => None,
    };
    let rust = match &bench.rust {
        Some(path) => {
            let sha256 = hash_file(path)?;
            let expected_vpr = viper.as_ref().map(|v| v.sha256.as_str());
            let action = match rust_metrics_by_sha256(db, &sha256).await? {
                None => MetricsAction::Insert,
                Some(row) => action_for(rust_problems(row, expected_vpr)),
            };
            Some(FileMetricsStatus { sha256, action })
        }
        None => None,
    };
    Ok(BenchmarkMetricsStatus { viper, rust })
}

fn action_for(problems: Vec<&'static str>) -> MetricsAction {
    if problems.is_empty() {
        MetricsAction::UpToDate
    } else {
        MetricsAction::Update(problems)
    }
}

/// The names of the given columns (bound as variables of the same name) that
/// are NULL.
macro_rules! null_columns {
    ($($col:ident),* $(,)?) => {{
        let mut nulls = Vec::new();
        $(if $col.is_none() { nulls.push(stringify!($col)); })*
        nulls
    }};
}

// The two functions below destructure the whole row without `..`: a column
// added to the schema (and the regenerated entity) does not compile until it is
// checked here, or explicitly ignored with `_`.

/// The columns of a Viper row that are NULL.
fn viper_problems(row: viper_metrics::Model) -> Vec<&'static str> {
    let viper_metrics::Model {
        vpr_sha256: _,
        measured_at: _,
        metrics_commit: _,
        loc,
        lines,
        bytes,
        methods,
        functions,
        predicates,
        domains,
        domain_functions,
        axioms,
        fields,
        adts,
        stmts,
        exps,
        folds,
        unfolds,
        heap_update_exps,
        inhales,
        exhales,
        asserts,
        assumes,
        accs,
        quantifiers,
        labels,
        gotos,
        ifs,
        whiles,
        assigns,
        method_calls,
        function_apps,
        old_exps,
        contract_clauses,
    } = row;
    null_columns!(
        loc, lines, bytes, methods, functions, predicates, domains, domain_functions, axioms,
        fields, adts, stmts, exps, folds, unfolds, heap_update_exps, inhales, exhales, asserts,
        assumes, accs, quantifiers, labels, gotos, ifs, whiles, assigns, method_calls,
        function_apps, old_exps, contract_clauses,
    )
}

/// The columns of a Rust row that are NULL or, for `vpr_sha256`, not
/// `expected_vpr`. The per-function columns may be NULL only in a file without
/// functions (`fns = 0`).
fn rust_problems(row: rust_metrics::Model, expected_vpr: Option<&str>) -> Vec<&'static str> {
    let rust_metrics::Model {
        rs_sha256: _,
        vpr_sha256,
        measured_at: _,
        metrics_commit: _,
        loc,
        fns,
        structs,
        enums,
        fn_loc,
        paths,
        stmts,
        exprs,
        loops,
        ifs,
        early_returns,
        matches,
        match_arms,
        args,
        args_by_value,
        args_by_ref,
        args_by_mut_ref,
        arg_fields,
        ret_fields,
        calls,
        distinct_callees,
        calls_in_loops,
        calls_in_branches,
        assigns,
        compound_assigns,
        mut_ref_writes,
        reborrows,
        struct_borrows,
        arith_ops,
        divisions,
        indexing,
        casts,
        unwraps,
        panics,
        enums_touched,
        generic_insts,
        cyclomatic,
        max_loop_depth,
        max_if_depth,
        max_seq_branches,
        max_branch_depth,
        max_match_arms,
        max_match_depth,
        max_enum_variants,
        max_payload_depth,
        max_call_depth,
        max_write_path,
        max_struct_depth,
    } = row;
    let mut problems = Vec::new();
    if vpr_sha256.as_deref() != expected_vpr {
        problems.push("vpr_sha256");
    }
    let has_fns = fns != Some(0);
    problems.extend(null_columns!(loc, fns, structs, enums));
    if has_fns {
        problems.extend(null_columns!(
            fn_loc, paths, stmts, exprs, loops, ifs, early_returns, matches, match_arms, args,
            args_by_value, args_by_ref, args_by_mut_ref, arg_fields, ret_fields, calls,
            distinct_callees, calls_in_loops, calls_in_branches, assigns, compound_assigns,
            mut_ref_writes, reborrows, struct_borrows, arith_ops, divisions, indexing, casts,
            unwraps, panics, enums_touched, generic_insts, cyclomatic, max_loop_depth,
            max_if_depth, max_seq_branches, max_branch_depth, max_match_arms, max_match_depth,
            max_enum_variants, max_payload_depth, max_call_depth, max_write_path,
            max_struct_depth,
        ));
    }
    problems
}
