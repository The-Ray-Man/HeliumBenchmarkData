//! `Display` for the runner's types, short enough for one log line each.
//! `Debug` stays the full dump; these are what the log shows.

use std::fmt;

use crate::db::queries::{BenchmarkMetricsStatus, FileMetricsStatus, MetricsAction};
use crate::executor::helium::BenchRun;
use crate::executor::{Measurement, MemberResult, Outcome, RawRun, RunContext, Verdict};
use crate::explorer::Benchmark;
use crate::RunStats;

/// Seconds with millisecond precision, e.g. `1.234s`.
struct Secs(f64);

impl fmt::Display for Secs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3}s", self.0)
    }
}

/// `suite/family/test`, or `suite/test` for a flat suite, plus which files
/// it has: `[rs+vpr]`, `[rs]` or `[vpr]`.
impl fmt::Display for Benchmark {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/", self.suite)?;
        if let Some(family) = &self.family {
            write!(f, "{family}/")?;
        }
        let files = match (self.rust.is_some(), self.viper.is_some()) {
            (true, true) => "rs+vpr",
            (true, false) => "rs",
            (false, true) => "vpr",
            (false, false) => "no files",
        };
        write!(f, "{} [{files}]", self.test)
    }
}

/// The verdict in lowercase; of a crash only the first line of its message.
impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Verdict::Verified => f.write_str("verified"),
            Verdict::Failed => f.write_str("failed"),
            Verdict::Inconclusive => f.write_str("inconclusive"),
            Verdict::Crashed(msg) => match msg.lines().next().filter(|l| !l.trim().is_empty()) {
                Some(line) => write!(f, "crashed ({})", line.trim()),
                None => f.write_str("crashed"),
            },
            Verdict::TimedOut => f.write_str("timed out"),
            Verdict::Skipped => f.write_str("skipped"),
        }
    }
}

/// `name: verdict (0.123s)`.
impl fmt::Display for MemberResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.verdict)?;
        if let Some(s) = self.seconds {
            write!(f, " ({})", Secs(s))?;
        }
        Ok(())
    }
}

/// `verdict, self 1.234s, 3 phases, 5 members (4 verified, 1 failed)`.
impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.verdict)?;
        if let Some(s) = self.self_time {
            write!(f, ", self {}", Secs(s))?;
        }
        if !self.phases.is_empty() {
            write!(f, ", {} phases", self.phases.len())?;
        }
        if !self.members.is_empty() {
            let count = |p: fn(&Verdict) -> bool| self.members.iter().filter(|m| p(&m.verdict)).count();
            write!(f, ", {} members (", self.members.len())?;
            let parts = [
                ("verified", count(|v| *v == Verdict::Verified)),
                ("failed", count(|v| *v == Verdict::Failed)),
                ("inconclusive", count(|v| *v == Verdict::Inconclusive)),
                ("crashed", count(|v| matches!(v, Verdict::Crashed(_)))),
                ("skipped", count(|v| *v == Verdict::Skipped)),
            ];
            let mut first = true;
            for (name, n) in parts.into_iter().filter(|(_, n)| *n > 0) {
                if !first {
                    f.write_str(", ")?;
                }
                write!(f, "{n} {name}")?;
                first = false;
            }
            f.write_str(")")?;
        }
        Ok(())
    }
}

/// `exit 0 after 1.234s`, `killed after 60.000s (timeout)`.
impl fmt::Display for RawRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.timed_out, self.exit_code) {
            (true, _) => write!(f, "killed after {} (timeout)", Secs(self.wall))?,
            (false, Some(code)) => write!(f, "exit {code} after {}", Secs(self.wall))?,
            (false, None) => write!(f, "no exit code after {}", Secs(self.wall))?,
        }
        if let Some(mb) = self.peak_rss_mb {
            write!(f, ", peak {mb:.1} MB")?;
        }
        Ok(())
    }
}

/// `helium: verified, self 1.0s (exit 0 after 1.234s)`.
impl fmt::Display for Measurement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} ({})", self.tool, self.outcomes, self.runs)
    }
}

/// `1 warm-up + 3 timed runs, timeout 60s, in <scratch>`.
impl fmt::Display for RunContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} warm-up + {} timed runs, ", self.warmup, self.runs)?;
        match self.timeout {
            Some(t) => write!(f, "timeout {}", Secs(t.as_secs_f64()))?,
            None => f.write_str("no timeout")?,
        }
        if self.force_rerun {
            f.write_str(", forced rerun")?;
        }
        write!(f, ", in {}", self.scratch.display())
    }
}

/// `<run id> on <host>, started <time>`.
impl fmt::Display for BenchRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.run_id)?;
        if let Some(host) = &self.host {
            write!(f, " on {host}")?;
        }
        write!(f, ", started {}", self.started_at.format("%Y-%m-%d %H:%M:%S %:z"))
    }
}

/// `insert`, `update (loc, fns)`, `up to date`.
impl fmt::Display for MetricsAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetricsAction::Insert => f.write_str("insert"),
            MetricsAction::Update(cols) => write!(f, "update ({})", cols.join(", ")),
            MetricsAction::UpToDate => f.write_str("up to date"),
        }
    }
}

/// `<first 12 hex digits of the hash>: <action>`.
impl fmt::Display for FileMetricsStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let short: String = self.sha256.chars().take(12).collect();
        write!(f, "{short}: {}", self.action)
    }
}

/// `vpr <status>, rs <status>`, leaving out a file the benchmark lacks.
impl fmt::Display for BenchmarkMetricsStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.viper, &self.rust) {
            (Some(v), Some(r)) => write!(f, "vpr {v}, rs {r}"),
            (Some(v), None) => write!(f, "vpr {v}"),
            (None, Some(r)) => write!(f, "rs {r}"),
            (None, None) => f.write_str("no files"),
        }
    }
}

/// `12 benchmarks, 30 new timed runs, 1 errors`.
impl fmt::Display for RunStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} benchmarks, {} new timed runs, {} errors", self.benchmarks, self.runs, self.errors)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn benchmark_names_suite_family_and_files() {
        let mut b = Benchmark {
            suite: "fam".into(),
            family: Some("f1".into()),
            test: "sub/x".into(),
            rust: Some(PathBuf::from("x.rs")),
            viper: Some(PathBuf::from("x.vpr")),
        };
        assert_eq!(b.to_string(), "fam/f1/sub/x [rs+vpr]");
        b.family = None;
        b.rust = None;
        assert_eq!(b.to_string(), "fam/sub/x [vpr]");
    }

    #[test]
    fn crash_shows_first_line_only() {
        let v = Verdict::Crashed("stack overflow\nat line 3".into());
        assert_eq!(v.to_string(), "crashed (stack overflow)");
        assert_eq!(Verdict::Crashed(String::new()).to_string(), "crashed");
    }

    #[test]
    fn outcome_counts_members() {
        let member = |verdict| MemberResult { name: "m".into(), verdict, seconds: None, message: None };
        let o = Outcome {
            verdict: Verdict::Failed,
            self_time: Some(1.5),
            phases: BTreeMap::new(),
            members: vec![member(Verdict::Verified), member(Verdict::Verified), member(Verdict::Failed)],
        };
        assert_eq!(o.to_string(), "failed, self 1.500s, 3 members (2 verified, 1 failed)");
    }

    #[test]
    fn metrics_action_lists_columns() {
        assert_eq!(MetricsAction::Update(vec!["loc", "fns"]).to_string(), "update (loc, fns)");
    }
}
