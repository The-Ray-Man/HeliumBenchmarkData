//! Running the measured tools (rustc, Helium, Silicon) on one benchmark.
//!
//! Every tool implements [`Executor`]: which input of a [`Benchmark`] it
//! takes, how to run it once, and how to read what that run printed. Running
//! and parsing are separate steps, so a parser can be tested on saved output
//! and a failed parse can be redone from the raw output kept on disk.
//! [`measure`] is the same for every tool: untimed warm-up runs (unless the
//! tool opts out), then timed runs, stopping at the first timeout or failed
//! parse.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use log::{debug, info, trace, warn};
use sea_orm::{ConnectionTrait, DbErr};
use crate::executor::helium::BenchRun;
use crate::explorer::Benchmark;
use crate::utils::hash_file;

pub mod helium;
pub mod rustc;
pub mod silicon;

/// A measured tool.
pub trait Executor {
    /// Short stable name, used as the tool's key in the database (`rustc`,
    /// `helium`, `silicon`).
    fn name(&self) -> &'static str;

    /// What identifies this exact build of the tool (version line, binary or
    /// jar hash), recorded with every result.
    fn id(&self) -> &str;

    /// The file of `bench` this tool runs on, or `None` when the benchmark
    /// has no such file (rustc needs the `.rs`, the verifiers the `.vpr`).
    fn input<'a>(&self, bench: &'a Benchmark) -> Option<&'a Path>;

    /// Whether [`measure`] does `RunContext::warmup` untimed runs of each
    /// file before the timed ones. Silicon is warmed up on its own corpus
    /// instead.
    fn per_file_warmup(&self) -> bool {
        true
    }

    /// How many runs of this exact build ([`Executor::id`]) on the input with
    /// SHA-256 `sha256` are already in the database.
    async fn check_existing<C: ConnectionTrait>(&self, conn: &C, sha256: &str) -> Result<usize, DbErr>;

    /// Whether a run of this exact build on the input with SHA-256 `sha256`
    /// already timed out, which makes [`measure`] skip the input like one
    /// with all its runs: a timeout ends a measurement, so the input has
    /// fewer runs, and measuring it again would most likely time out again.
    /// Helium keeps the default (`false`): its timed-out files are measured
    /// again, since a new commit may make them finish.
    async fn timed_out_before<C: ConnectionTrait>(&self, _conn: &C, _sha256: &str) -> Result<bool, DbErr> {
        Ok(false)
    }

    /// Run the tool once on `input`. Output goes to files under
    /// `ctx.scratch`; a run that does not finish within `ctx.timeout` is
    /// killed and returned with `timed_out` set.
    fn run(&mut self, input: &Path, ctx: &RunContext) -> std::io::Result<RawRun>;

    /// Read a finished run. Never called on a timed-out run, whose output
    /// may be incomplete.
    fn parse(&self, raw: &RawRun) -> Result<Outcome, ParseError>;

    /// Stores the result in the database
    async fn store<C: ConnectionTrait>(
        &self,
        conn: &C,
        run: &BenchRun,
        bench: &Benchmark,
        ctx: &RunContext,
        measurements: &[Measurement],
    ) -> Result<(), DbErr>;
}

/// Settings shared by every run of a measurement.
#[derive(Debug, Clone)]
pub struct RunContext {
    /// Directory for this measurement's output files.
    pub scratch: PathBuf,
    /// `None`: no limit.
    pub timeout: Option<Duration>,
    /// Untimed runs before the timed ones.
    pub warmup: usize,
    /// Timed runs.
    pub runs: usize,
    /// Forced rerun
    pub force_rerun: bool,
}

/// What one process run left behind, before any parsing.
#[derive(Debug, Clone)]
pub struct RawRun {
    /// Seconds from start to exit (or to the kill, on timeout).
    pub wall: f64,
    pub peak_rss_mb: Option<f64>,
    /// `None` when killed.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// The verdict of one run, as the tool reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Verified successfully
    Verified,
    /// Could not be verified (e.g. some assertion did not hold)
    Failed,
    /// We can not make a statement about it, because we miss some features
    Inconclusive,
    /// The program crashed (e.g. Stack Overflow)
    Crashed(String),
    /// The program was killed because of a timeout
    TimedOut,
    /// The test was skipped
    Skipped,
}

/// One member's (method, function, ...) verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberResult {
    pub name: String,
    pub verdict: Verdict,
    /// The tool's own time for this member, if it reports one.
    pub seconds: Option<f64>,
    pub message: Option<String>,
}

/// What one run reported about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub verdict: Verdict,
    /// The tool's own total, without process (or JVM) startup.
    pub self_time: Option<f64>,
    /// Seconds per phase, in the tool's own names.
    pub phases: BTreeMap<String, f64>,
    pub members: Vec<MemberResult>,
}

/// The output of a run could not be read.
#[derive(Debug, Clone)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "could not parse the output: {}", self.0)
    }
}

impl std::error::Error for ParseError {}


/// One tool on one benchmark: the timed runs and what each reported.
#[derive(Debug, Clone)]
pub struct Measurement {
    pub tool: &'static str,
    pub tool_id: String,
    /// Timed runs only, in order. On a timeout or parse error, the last one
    /// is the run that stopped the measurement.
    pub runs: RawRun,
    /// One per timed run that parsed.
    pub outcomes: Outcome,
}

/// Measure `exec` on `bench`: `ctx.warmup` untimed runs, then `ctx.runs`
/// timed ones. `None` when the benchmark has no input for this tool.
pub async fn measure<C: ConnectionTrait, E: Executor>(
    conn: &C,
    exec: &mut E,
    bench: &Benchmark,
    ctx: &RunContext,
) -> std::io::Result<Vec<Measurement>> {
    // This benchmark does not have a file of the correct type
    let Some(input) = exec.input(bench) else {
        debug!("{bench}: no input for {}, skipped", exec.name());
        return Ok(vec![]);
    };

    let sha256 = hash_file(input)?;

    // Check if we actually need to compute it or if we already have the data
    if !ctx.force_rerun {
        let existing = exec.check_existing(conn, &sha256).await.map_err(std::io::Error::other)?;
        if existing >= ctx.runs {
            debug!("{bench}: {} already has {existing} runs, skipped", exec.name());
            return Ok(vec![]);
        }
        if exec.timed_out_before(conn, &sha256).await.map_err(std::io::Error::other)? {
            debug!("{bench}: {} already timed out, skipped", exec.name());
            return Ok(vec![]);
        }
        debug!("{bench}: {} has {existing} of {} runs", exec.name(), ctx.runs);
    }
    info!("{bench}: measuring {} on {}", exec.name(), input.display());

    // warmup run
    let warmup = if exec.per_file_warmup() { ctx.warmup } else { 0 };
    for i in 0..warmup {
        match exec.run(input, ctx) {
            Ok(raw) => debug!("{bench}: {} warm-up {}/{warmup}: {raw}", exec.name(), i + 1),
            Err(err) => warn!("{bench}: {} warm-up run failed: {err}", exec.name()),
        }
    }

    let tool_id = exec.id().to_string();
    let tool = exec.name();

    // actual runs
    let mut measurements = vec![];
    for i in 0..ctx.runs {
        let raw = exec.run(input, ctx);
        match raw {
            Ok(raw_run) if raw_run.timed_out => {
                warn!("{bench}: {tool} run {}/{}: {raw_run}, remaining runs skipped", i + 1, ctx.runs);
                // Output of a killed run may be incomplete: not parsed, and
                // the remaining runs would time out as well.
                measurements.push(Measurement {
                    tool,
                    tool_id: tool_id.clone(),
                    runs: raw_run,
                    outcomes: verdict_only(Verdict::TimedOut),
                });
                break;
            }
            Ok(raw_run) => {
                let outcome = exec.parse(&raw_run).unwrap_or_else(|err| {
                    warn!("{bench}: {tool} run {}/{}: {err} ({raw_run})", i + 1, ctx.runs);
                    // Usually a crash before the report (e.g. a stack
                    // overflow); its last lines on stderr say why.
                    let tail: Vec<&str> = raw_run.stderr.lines().rev().take(5).collect();
                    let tail: Vec<&str> = tail.into_iter().rev().collect();
                    verdict_only(Verdict::Crashed(format!("{err}\n{}", tail.join("\n"))))
                });
                let m = Measurement {
                    tool,
                    tool_id: tool_id.clone(),
                    runs: raw_run,
                    outcomes: outcome,
                };
                debug!("{bench}: run {}/{}: {m}", i + 1, ctx.runs);
                measurements.push(m)
            }
            Err(err) => {
                warn!("{bench}: {tool} run {}/{} could not start: {err}", i + 1, ctx.runs);
                measurements.push(Measurement {
                    tool,
                    tool_id: tool_id.clone(),
                    runs: RawRun {
                        wall: 0.0,
                        peak_rss_mb: None,
                        exit_code: Some(1),
                        stdout: "".to_string(),
                        stderr: err.to_string(),
                        timed_out: false,
                    },
                    outcomes: verdict_only(Verdict::Crashed(err.to_string())),
                })
            }
        }
    }

    if let Some(last) = measurements.last() {
        let mean = measurements.iter().map(|m| m.runs.wall).sum::<f64>() / measurements.len() as f64;
        info!("{bench}: {last}, mean wall {mean:.3}s over {} runs", measurements.len());
    }
    Ok(measurements)
}

/// An outcome with nothing but its verdict.
fn verdict_only(verdict: Verdict) -> Outcome {
    Outcome {
        verdict,
        self_time: None,
        phases: BTreeMap::new(),
        members: vec![],
    }
}

/// Run `cmd` as its own process, stdout and stderr redirected to files in
/// `scratch` (a chatty process cannot block on a full pipe while it is
/// timed), and kill it on `timeout`.
pub fn run_process(
    cmd: &mut Command,
    timeout: Option<Duration>,
    scratch: &Path,
) -> std::io::Result<RawRun> {
    std::fs::create_dir_all(scratch)?;
    let out_path = scratch.join("stdout.txt");
    let err_path = scratch.join("stderr.txt");
    cmd.stdin(Stdio::null())
        .stdout(File::create(&out_path)?)
        .stderr(File::create(&err_path)?);

    trace!("running {cmd:?}");
    let start = Instant::now();
    let mut child = cmd.spawn()?;
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (Some(status), false);
        }
        if timeout.is_some_and(|t| start.elapsed() >= t) {
            child.kill()?;
            child.wait()?;
            break (None, true);
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let wall = start.elapsed().as_secs_f64();
    let read = |p: &Path| -> std::io::Result<String> {
        Ok(String::from_utf8_lossy(&std::fs::read(p)?).into_owned())
    };
    Ok(RawRun {
        wall,
        peak_rss_mb: None,
        exit_code: status.and_then(|s| s.code()),
        stdout: read(&out_path)?,
        stderr: read(&err_path)?,
        timed_out,
    })
}
