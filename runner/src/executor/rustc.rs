//! rustc checking one `.rs` file (`--emit=metadata`): the baseline Helium's
//! verification time is compared against.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use log::debug;
use regex::Regex;
use sea_orm::ActiveValue::Set;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, PaginatorTrait, QueryFilter};

use serde_json::json;

use crate::db::rustc_runs;
use crate::db::sea_orm_active_enums::RunStatus;
use super::helium::{BenchRun, run_status};
use super::{Executor, Measurement, Outcome, ParseError, RawRun, RunContext, Verdict, run_process};
use crate::explorer::Benchmark;
use crate::utils::hash_file;

pub struct Rustc {
    /// The toolchain's own `rustc`, not the rustup proxy (which adds its own
    /// process and tens of milliseconds to every run).
    rustc: PathBuf,
    args: Vec<String>,
    /// The first line of `rustc -vV`.
    id: String,
    /// Nightly rustc gets `-Z time-passes`, so it reports its own times.
    time_passes: bool,
}

impl Rustc {
    /// `toolchain` is passed to rustup as `+toolchain`; `args` go before the
    /// file on every run (e.g. `--edition 2021`). Without a `--crate-type`
    /// in `args`, files are checked as `--crate-type lib`: most benchmarks
    /// have no `main`.
    pub fn new(toolchain: Option<&str>, mut args: Vec<String>) -> std::io::Result<Self> {
        if !args.iter().any(|a| a.starts_with("--crate-type")) {
            args.extend(["--crate-type".to_string(), "lib".to_string()]);
        }
        let proxy = |extra: &[&str]| {
            let mut c = Command::new("rustc");
            if let Some(tc) = toolchain {
                c.arg(format!("+{tc}"));
            }
            c.args(extra);
            c
        };
        let out = proxy(&["--print", "sysroot"]).output()?;
        if !out.status.success() {
            return Err(std::io::Error::other(format!(
                "rustc --print sysroot: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let exe = if cfg!(windows) { "rustc.exe" } else { "rustc" };
        let rustc = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())
            .join("bin")
            .join(exe);
        let version = Command::new(&rustc).arg("-vV").output()?;
        let id = String::from_utf8_lossy(&version.stdout)
            .lines()
            .next()
            .unwrap_or("rustc")
            .trim()
            .to_string();
        let time_passes = id.contains("-nightly") || id.contains("-dev");
        debug!(
            "rustc at {}, args {args:?}, {}",
            rustc.display(),
            if time_passes { "with -Z time-passes" } else { "no -Z time-passes (not nightly)" }
        );
        Ok(Rustc { rustc, args, id, time_passes })
    }
}

impl Executor for Rustc {
    fn name(&self) -> &'static str {
        "rustc"
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn input<'a>(&self, bench: &'a Benchmark) -> Option<&'a Path> {
        bench.rust.as_deref()
    }

    async fn check_existing<C: ConnectionTrait>(&self, conn: &C, sha256: &str) -> Result<usize, DbErr> {
        rustc_runs::Entity::find()
            .filter(rustc_runs::Column::BackendVersion.eq(&self.id))
            .filter(rustc_runs::Column::RsSha256.eq(sha256))
            .count(conn)
            .await
            .map(|n| n as usize)
    }

    async fn timed_out_before<C: ConnectionTrait>(&self, conn: &C, sha256: &str) -> Result<bool, DbErr> {
        rustc_runs::Entity::find()
            .filter(rustc_runs::Column::BackendVersion.eq(&self.id))
            .filter(rustc_runs::Column::RsSha256.eq(sha256))
            .filter(rustc_runs::Column::Status.eq(RunStatus::TimedOut))
            .count(conn)
            .await
            .map(|n| n > 0)
    }

    fn run(&mut self, input: &Path, ctx: &RunContext) -> std::io::Result<RawRun> {
        let mut cmd = Command::new(&self.rustc);
        cmd.args(&self.args)
            .arg("--emit=metadata")
            .arg("--out-dir")
            .arg(ctx.scratch.join("rustc-out"));
        if self.time_passes {
            cmd.arg("-Ztime-passes");
        }
        cmd.arg(input);
        run_process(&mut cmd, ctx.timeout, &ctx.scratch)
    }

    /// On nightly, `self_time` is rustc's own `total` and `phases` its
    /// passes (nested ones included, so they do not add up to the total).
    fn parse(&self, raw: &RawRun) -> Result<Outcome, ParseError> {
        let verdict = match raw.exit_code {
            Some(0) => Verdict::Verified,
            Some(_) => Verdict::Failed,
            None => return Err(ParseError("rustc was killed".into())),
        };
        let (self_time, phases) = if self.time_passes {
            parse_time_passes(&raw.stderr)?
        } else {
            (None, BTreeMap::new())
        };
        if self.time_passes && verdict == Verdict::Verified && self_time.is_none() {
            return Err(ParseError("no `-Z time-passes` total".into()));
        }
        Ok(Outcome {
            verdict,
            self_time,
            phases,
            members: Vec::new(),
        })
    }

    /// Insert the timed runs of one [`super::measure`] of `bench` into
    /// `rustc_runs`, numbered on from the runs already stored. The
    /// benchmark's metrics row must exist already (foreign key on the hash).
    async fn store<C: ConnectionTrait>(
        &self,
        conn: &C,
        run: &BenchRun,
        bench: &Benchmark,
        ctx: &RunContext,
        measurements: &[Measurement],
    ) -> Result<(), DbErr> {
        let Some(rs) = bench.rust.as_deref() else {
            return Ok(());
        };
        if measurements.is_empty() {
            return Ok(());
        }
        let rs_sha256 = hash_file(rs)
            .map_err(|e| DbErr::Custom(format!("cannot read {}: {e}", rs.display())))?;
        let existing_runs = self.check_existing(conn, &rs_sha256).await.unwrap_or(0);
        let rows = measurements.iter().enumerate().map(|(i, m)| {
            // A failed check says why in its first `error` line.
            let message = match &m.outcomes.verdict {
                Verdict::Crashed(msg) if !msg.is_empty() => Some(msg.clone()),
                Verdict::Failed => m
                    .runs
                    .stderr
                    .lines()
                    .find(|l| l.starts_with("error"))
                    .map(str::to_string),
                _ => None,
            };
            rustc_runs::ActiveModel {
                run_id: Set(run.run_id.clone()),
                started_at: Set(run.started_at),
                host: Set(run.host.clone()),
                suite: Set(bench.suite.clone()),
                family: Set(bench.family.clone()),
                test: Set(bench.test.clone()),
                rs_sha256: Set(rs_sha256.clone()),
                backend_version: Set(Some(self.id.clone())),
                iteration: Set((existing_runs + i) as i32),
                status: Set(run_status(&m.outcomes.verdict)),
                timeout_s: Set(ctx.timeout.map(|t| t.as_secs_f64())),
                wall_s: Set(Some(m.runs.wall)),
                peak_rss_mb: Set(m.runs.peak_rss_mb),
                message: Set(message),
                meta: Set(Some(json!({
                    "warmup": ctx.warmup,
                    "exit_code": m.runs.exit_code,
                    "args": self.args,
                }))),
                self_s: Set(m.outcomes.self_time),
                phases: Set((!m.outcomes.phases.is_empty()).then(|| json!(m.outcomes.phases))),
                ..Default::default()
            }
        });
        rustc_runs::Entity::insert_many(rows).exec(conn).await?;
        debug!("{bench}: stored {} rustc runs from iteration {existing_runs}", measurements.len());
        Ok(())
    }
}

/// One `-Z time-passes` line:
/// `time:   0.071; rss:   26MB ->   31MB (   +5MB) type_check_crate`
/// (tab before the pass name).
static PASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^time:\s*([0-9.]+);.*?\s(\S+)\s*$").unwrap()
});

/// The `total` line and every other pass, a pass seen more than once
/// (`drop_ast`) summed.
fn parse_time_passes(err: &str) -> Result<(Option<f64>, BTreeMap<String, f64>), ParseError> {
    let mut total = None;
    let mut phases = BTreeMap::new();
    for c in PASS.captures_iter(err) {
        let secs: f64 = c[1]
            .parse()
            .map_err(|e| ParseError(format!("time-passes `{}`: {e}", &c[2])))?;
        match &c[2] {
            "total" => total = Some(secs),
            name => *phases.entry(name.to_string()).or_insert(0.0) += secs,
        }
    }
    Ok((total, phases))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_time_passes() {
        let err = "time:   0.015; rss:   18MB ->   19MB (   +1MB)\tparse_crate\r\n\
                   time:   0.001; rss:   25MB ->   25MB (   +0MB)\tdrop_ast\n\
                   time:   0.002; rss:   25MB ->   25MB (   +0MB)\tdrop_ast\n\
                   error[E0308]: mismatched types\n\
                   time:   0.575; rss:   14MB ->   28MB (  +14MB)\ttotal\n";
        let (total, phases) = parse_time_passes(err).unwrap();
        assert_eq!(total, Some(0.575));
        assert_eq!(phases.len(), 2);
        assert_eq!(phases["parse_crate"], 0.015);
        assert!((phases["drop_ast"] - 0.003).abs() < 1e-9);
    }
}
