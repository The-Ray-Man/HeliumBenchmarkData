//! rustc checking one `.rs` file (`--emit=metadata`): the baseline Helium's
//! verification time is compared against.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, PaginatorTrait, QueryFilter};

use crate::db::rustc_runs;
use super::{Executor, Outcome, ParseError, RawRun, RunContext, Verdict, run_process};
use crate::explorer::Benchmark;

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
    /// file on every run (e.g. `--edition 2021 --crate-type lib`).
    pub fn new(toolchain: Option<&str>, args: Vec<String>) -> std::io::Result<Self> {
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
