//! Helium's `verify`, run as `verify --json <file.vpr>`: one JSON line on
//! stdout with the per-member results, phase times and the pipeline total.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use chrono::{DateTime, FixedOffset};
use sea_orm::ActiveValue::{self, Set};
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, PaginatorTrait, QueryFilter};
use serde_json::{json, Value};
use crate::db::helium_runs;
use crate::db::sea_orm_active_enums::RunStatus;
use super::{run_process, Executor, Measurement, MemberResult, Outcome, ParseError, RawRun, RunContext, Verdict};
use crate::explorer::Benchmark;
use crate::utils::hash_file;

pub struct Helium {
    binary: PathBuf,
    /// What `verify --version` prints, ending in the commit the binary was
    /// built from, with `-dirty` appended when the checkout had local changes.
    id: String,
    /// `id` without `-dirty`: the `backend_version` column.
    commit: String,
    /// Built from uncommitted changes on top of `commit`.
    dirty: bool,
}

impl Helium {
    /// `binary` is `<crate>/target/<profile>/verify[.exe]`. It is rebuilt
    /// first (`cargo [+toolchain] build --profile <profile> --bin verify` in
    /// `<crate>`), so the measured build matches the checkout, then asked for
    /// its version. `toolchain` is passed to rustup as `+toolchain`; `None`
    /// uses the default resolution.
    pub fn new(binary: PathBuf, toolchain: Option<&str>) -> std::io::Result<Self> {
        rebuild(&binary, toolchain)?;
        let out = Command::new(&binary).arg("--version").output()?;
        if !out.status.success() {
            return Err(std::io::Error::other(format!(
                "{} --version: {}",
                binary.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // `helium-build silver-oxide 0.1.0 commit <hash>[-dirty]`
        let describe = id.split_whitespace().last().unwrap_or_default();
        let (commit, dirty) = match describe.strip_suffix("-dirty") {
            Some(commit) => (commit.to_string(), true),
            None => (describe.to_string(), false),
        };
        Ok(Helium { binary, id, commit, dirty })
    }
}

/// What every row of one benchmark run (one start of the runner) shares.
#[derive(Debug, Clone)]
pub struct BenchRun {
    /// `<date>_<first 12 characters of the commit>`, e.g. `2026-10-06_bd5a136f97a5`.
    pub run_id: String,
    pub started_at: DateTime<FixedOffset>,
    pub host: Option<String>,
}

impl BenchRun {
    pub fn new(helium: &Helium) -> Self {
        let started_at = chrono::Local::now().fixed_offset();
        let short: String = helium.commit.chars().take(12).collect();
        BenchRun {
            run_id: format!("{}_{short}", started_at.format("%Y-%m-%d")),
            started_at,
            host: std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok(),
        }
    }
}

pub(super) fn run_status(v: &Verdict) -> RunStatus {
    match v {
        Verdict::Verified => RunStatus::Verified,
        Verdict::Failed => RunStatus::Failed,
        Verdict::Inconclusive => RunStatus::Inconclusive,
        Verdict::Crashed(_) => RunStatus::Crashed,
        Verdict::TimedOut => RunStatus::TimedOut,
        Verdict::Skipped => RunStatus::Skipped,
    }
}

/// The `stats` object of `verify --json`. Each field is removed as it is
/// moved into its column, so [`Stats::rest`] holds only the unmapped ones.
struct Stats(serde_json::Map<String, Value>);

impl Stats {
    fn new(json: Option<&Value>) -> Self {
        let stats = json.and_then(|j| j.get("stats")).and_then(Value::as_object);
        Stats(stats.cloned().unwrap_or_default())
    }

    /// Remove the value at `path` if `convert` accepts it; a value of the
    /// wrong type stays behind for `meta`.
    fn take<T>(&mut self, path: &[&str], convert: impl FnOnce(&Value) -> Option<T>) -> Option<T> {
        let (last, parents) = path.split_last()?;
        let mut map = &mut self.0;
        for p in parents {
            map = map.get_mut(*p)?.as_object_mut()?;
        }
        let value = convert(map.get(*last)?)?;
        map.remove(*last);
        Some(value)
    }

    fn seconds(&mut self, path: &[&str]) -> ActiveValue<Option<f64>> {
        Set(self.take(path, Value::as_f64))
    }

    fn count(&mut self, key: &str) -> ActiveValue<Option<i64>> {
        Set(self.take(&[key], Value::as_i64))
    }

    fn object(&mut self, key: &str) -> ActiveValue<Option<Value>> {
        Set(self.take(&[key], |v| v.is_object().then(|| v.clone())))
    }

    /// What no column took, without the groups that were emptied.
    fn rest(mut self) -> Option<Value> {
        self.0.retain(|_, v| v.as_object().is_none_or(|o| !o.is_empty()));
        (!self.0.is_empty()).then(|| Value::Object(self.0))
    }
}

/// The JSON line `verify --json` prints on stdout.
fn json_line(stdout: &str) -> Option<Value> {
    let line = stdout.lines().find(|l| l.trim_start().starts_with('{'))?;
    serde_json::from_str(line).ok()
}

/// Rebuild `binary` with cargo, reading the crate and the profile from its
/// path (`<crate>/target/<profile>/<bin>`), with `toolchain` if given.
fn rebuild(binary: &Path, toolchain: Option<&str>) -> std::io::Result<()> {
    let bad_path = || {
        std::io::Error::other(format!(
            "{}: expected `<crate>/target/<profile>/<binary>`",
            binary.display()
        ))
    };
    let binary = std::path::absolute(binary)?;
    let bin = binary.file_stem().and_then(|s| s.to_str()).ok_or_else(bad_path)?;
    let profile_dir = binary.parent().ok_or_else(bad_path)?;
    let target_dir = profile_dir.parent().filter(|t| t.ends_with("target")).ok_or_else(bad_path)?;
    let crate_dir = target_dir.parent().ok_or_else(bad_path)?;
    // cargo calls the `debug` output folder's profile `dev`.
    let profile = match profile_dir.file_name().and_then(|s| s.to_str()).ok_or_else(bad_path)? {
        "debug" => "dev",
        p => p,
    };
    let mut cmd = Command::new("cargo");
    if let Some(tc) = toolchain {
        cmd.arg(format!("+{tc}"));
    }
    let status = cmd
        .args(["build", "--profile", profile, "--bin", bin])
        .current_dir(crate_dir)
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "cargo build of `{bin}` in {} failed ({status})",
            crate_dir.display()
        )));
    }
    Ok(())
}

impl Executor for Helium {
    fn name(&self) -> &'static str {
        "helium"
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn input<'a>(&self, bench: &'a Benchmark) -> Option<&'a Path> {
        bench.viper.as_deref()
    }

    async fn check_existing<C: ConnectionTrait>(&self, conn: &C, sha256: &str) -> Result<usize, DbErr> {
        helium_runs::Entity::find()
            .filter(helium_runs::Column::BackendVersion.eq(&self.commit))
            .filter(helium_runs::Column::Dirty.eq(self.dirty))
            .filter(helium_runs::Column::VprSha256.eq(sha256))
            .count(conn)
            .await
            .map(|n| n as usize)
    }

    /// Insert the timed runs of one [`super::measure`] of `bench` into
    /// `helium_runs`, numbered on from the runs already stored. The benchmark's metrics rows must
    /// exist already (foreign keys on both hashes).
    async fn store<C: ConnectionTrait>(
        &self,
        conn: &C,
        run: &BenchRun,
        bench: &Benchmark,
        ctx: &RunContext,
        measurements: &[Measurement],
    ) -> Result<(), DbErr> {
        let Some(vpr) = bench.viper.as_deref() else {
            return Ok(());
        };
        if measurements.is_empty() {
            return Ok(());
        }
        let hash = |p: &Path| {
            hash_file(p).map_err(|e| DbErr::Custom(format!("cannot read {}: {e}", p.display())))
        };
        let vpr_sha256 = hash(vpr)?;
        let rs_sha256 = bench.rust.as_deref().map(hash).transpose()?;
        let existing_runs = self.check_existing(conn, &vpr_sha256).await.unwrap_or(0);
        let rows = measurements.iter().enumerate().map(|(i, m)| {
            let json = json_line(&m.runs.stdout);
            let mut stats = Stats::new(json.as_ref());
            let phase = |name: &str| Set(m.outcomes.phases.get(name).copied());
            let count = |f: fn(&Verdict) -> bool| {
                Set(Some(m.outcomes.members.iter().filter(|x| f(&x.verdict)).count() as i32))
            };
            let message = match &m.outcomes.verdict {
                Verdict::Crashed(msg) if !msg.is_empty() => Some(msg.clone()),
                _ => None,
            };
            let mut row = helium_runs::ActiveModel {
                run_id: Set(run.run_id.clone()),
                started_at: Set(run.started_at),
                host: Set(run.host.clone()),
                suite: Set(bench.suite.clone()),
                family: Set(bench.family.clone()),
                test: Set(bench.test.clone()),
                vpr_sha256: Set(vpr_sha256.clone()),
                rs_sha256: Set(rs_sha256.clone()),
                backend_version: Set(Some(self.commit.clone())),
                dirty: Set(Some(self.dirty)),
                iteration: Set((existing_runs + i) as i32),
                status: Set(run_status(&m.outcomes.verdict)),
                timeout_s: Set(ctx.timeout.map(|t| t.as_secs_f64())),
                wall_s: Set(Some(m.runs.wall)),
                peak_rss_mb: Set(m
                    .runs
                    .peak_rss_mb
                    .or_else(|| json.as_ref()?["peak_rss_mb"].as_f64())),
                message: Set(message),
                self_s: Set(m.outcomes.self_time),
                members_ok: count(|v| *v == Verdict::Verified),
                members_fail: count(|v| *v == Verdict::Failed),
                members_unsupported: count(|v| *v == Verdict::Inconclusive),
                members_skip: count(|v| *v == Verdict::Skipped),
                members_error: count(|v| matches!(v, Verdict::Crashed(_))),
                parse_s: phase("parse"),
                macros_s: phase("macros"),
                idents_s: phase("idents"),
                disambiguate_s: phase("disambiguate"),
                globals_s: phase("globals"),
                typecheck_s: phase("typecheck"),
                analyze_s: phase("analyze"),
                translate_s: phase("translate"),
                verify_s: phase("verify"),
                search_s: stats.seconds(&["timing", "search"]),
                apply_s: stats.seconds(&["timing", "apply"]),
                rebuild_s: stats.seconds(&["timing", "rebuild"]),
                ground_s: stats.seconds(&["graph_timing", "ground"]),
                scratch_s: stats.seconds(&["graph_timing", "scratch"]),
                probe_s: stats.seconds(&["graph_timing", "probe"]),
                scratch_clone_s: stats.seconds(&["graph_timing", "scratch_clone"]),
                saturations: stats.count("saturations"),
                sat_iterations: stats.count("sat_iterations"),
                reduces: stats.count("reduces"),
                rule_applications: stats.count("rule_applications"),
                insts_processed: stats.count("insts_processed"),
                egraph_nodes_peak: stats.count("egraph_nodes_peak"),
                egraph_classes_peak: stats.count("egraph_classes_peak"),
                probe_saturations: stats.count("probe_saturations"),
                probe_iterations: stats.count("probe_iterations"),
                probe_reduces: stats.count("probe_reduces"),
                probe_early_stops: stats.count("probe_early_stops"),
                block_scratch_clones: stats.count("block_scratch_clones"),
                block_scratch_saturations: stats.count("block_scratch_saturations"),
                block_scratch_iterations: stats.count("block_scratch_iterations"),
                block_scratch_freehits: stats.count("block_scratch_freehits"),
                prove_calls: stats.count("prove_calls"),
                prove_goal_true: stats.count("prove_goal_true"),
                prove_inconsistent: stats.count("prove_inconsistent"),
                prove_dead_block: stats.count("prove_dead_block"),
                prove_memo: stats.count("prove_memo"),
                prove_saturate: stats.count("prove_saturate"),
                prove_probe: stats.count("prove_probe"),
                prove_ite_decompose: stats.count("prove_ite_decompose"),
                per_rule: stats.object("per_rule"),
                rule_timing: stats.object("rule_timing"),
                ..Default::default()
            };
            // Only the stats without a column of their own (e.g. counters
            // added to Helium after the schema) stay in `meta`.
            let mut meta = json!({ "warmup": ctx.warmup, "exit_code": m.runs.exit_code });
            if let Some(rest) = stats.rest() {
                meta["stats"] = rest;
            }
            row.meta = Set(Some(meta));
            row
        });
        helium_runs::Entity::insert_many(rows).exec(conn).await?;
        Ok(())
    }

    fn run(&mut self, input: &Path, ctx: &RunContext) -> std::io::Result<RawRun> {
        let mut cmd = Command::new(&self.binary);
        cmd.arg("--json").arg(input);
        run_process(&mut cmd, ctx.timeout, &ctx.scratch)
    }

    fn parse(&self, raw: &RawRun) -> Result<Outcome, ParseError> {
        let v = json_line(&raw.stdout).ok_or_else(|| ParseError("no JSON line on stdout".into()))?;
        parse_json(&v)
    }
}

/// `OK`, `FAIL`, `UNSUPPORTED`, `ERROR`, `SKIP` as printed by `verify`;
/// `detail` is the member's message, kept for `ERROR`.
fn verdict(tag: &str, detail: Option<&str>) -> Result<Verdict, ParseError> {
    match tag {
        "OK" => Ok(Verdict::Verified),
        "FAIL" => Ok(Verdict::Failed),
        "UNSUPPORTED" => Ok(Verdict::Inconclusive),
        "SKIP" => Ok(Verdict::Skipped),
        "ERROR" => Ok(Verdict::Crashed(detail.unwrap_or_default().to_owned())),
        other => Err(ParseError(format!("unknown member status `{other}`"))),
    }
}

fn parse_json(v: &Value) -> Result<Outcome, ParseError> {
    let num_map = |key: &str| -> BTreeMap<String, f64> {
        v[key]
            .as_object()
            .map(|o| o.iter().filter_map(|(k, x)| Some((k.clone(), x.as_f64()?))).collect())
            .unwrap_or_default()
    };
    let times = num_map("member_times");
    let mut members = Vec::new();
    for row in v["results"].as_array().into_iter().flatten() {
        let name = row["name"]
            .as_str()
            .ok_or_else(|| ParseError("result without a name".into()))?;
        let status = row["status"]
            .as_str()
            .ok_or_else(|| ParseError(format!("`{name}` without a status")))?;
        let detail = row["detail"].as_str();
        members.push(MemberResult {
            name: name.to_string(),
            verdict: verdict(status, detail)?,
            seconds: times.get(name).copied(),
            message: detail.map(String::from),
        });
    }
    let verdict = if let Some(error) = v["error"].as_str() {
        Verdict::Crashed(error.to_string())
    } else if members.iter().any(|m| m.verdict == Verdict::Failed) {
        Verdict::Failed
    } else if members.iter().any(|m| m.verdict == Verdict::Inconclusive) {
        Verdict::Inconclusive
    } else if members.iter().all(|m| matches!(m.verdict, Verdict::Verified | Verdict::Skipped)) {
        Verdict::Verified
    } else {
        Verdict::Inconclusive
    };
    Ok(Outcome {
        verdict,
        self_time: v["total"].as_f64(),
        phases: num_map("phases"),
        members,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_results_and_times() {
        let v: Value = serde_json::from_str(
            r#"{"results":[{"name":"m_a","status":"OK"},{"name":"m_b","status":"FAIL"}],
                "phases":{"parse":0.5},"total":1.25,"member_times":{"m_a":0.1}}"#,
        )
        .unwrap();
        let o = parse_json(&v).unwrap();
        assert_eq!(o.verdict, Verdict::Failed);
        assert_eq!(o.self_time, Some(1.25));
        assert_eq!(o.phases["parse"], 0.5);
        assert_eq!(o.members[0].seconds, Some(0.1));
        assert_eq!(o.members[1].verdict, Verdict::Failed);
    }

    #[test]
    fn file_level_error() {
        let v: Value = serde_json::from_str(r#"{"results":[],"error":"parse error"}"#).unwrap();
        assert_eq!(parse_json(&v).unwrap().verdict, Verdict::Crashed("parse error".to_owned()));
    }

    #[test]
    fn stats_move_into_columns() {
        let v: Value = serde_json::from_str(
            r#"{"stats":{"timing":{"search":0.5,"apply":0.25},"reduces":105,
                "per_rule":{"eq-refl":1},"new_counter":7}}"#,
        )
        .unwrap();
        let mut stats = Stats::new(Some(&v));
        assert_eq!(stats.seconds(&["timing", "search"]), Set(Some(0.5)));
        assert_eq!(stats.seconds(&["timing", "apply"]), Set(Some(0.25)));
        assert_eq!(stats.seconds(&["timing", "rebuild"]), Set(None));
        assert_eq!(stats.count("reduces"), Set(Some(105)));
        assert_eq!(stats.object("per_rule"), Set(Some(json!({"eq-refl": 1}))));
        assert_eq!(stats.rest(), Some(json!({"new_counter": 7})));
    }

    #[test]
    fn no_stats_leave_nothing() {
        let mut stats = Stats::new(None);
        assert_eq!(stats.count("reduces"), Set(None));
        assert_eq!(stats.rest(), None);
    }
}
