//! Viper's Silicon on the same `.vpr` Helium verifies, in a warm JVM.
//!
//! In a cold `java -jar` run, Silicon's own clock starts before it creates
//! its verifier, so on small files its time is mostly class loading, the
//! JIT's first passes and Z3's start, about two seconds that do not depend on
//! the file. Cold runs are therefore not measured at all. One JVM runs every
//! file through Silicon's own command-line path (`SilFrontend.execute`, a
//! fresh verifier and Z3 per file, as a cold run), after it has been warmed
//! up on files of a separate corpus (`silicon.warm.corpus`, e.g. Silicon's own
//! test files), minus any file whose content is also a benchmark (by
//! SHA-256), so no benchmark file has been seen by the JIT before it is timed.
//! `self_time` is the time Silicon reports, read from its summary line; the
//! wall time is the driver's own clock around the file, without the protocol.
//!
//! The driver is `runner/silicon/SiliconWarm.java`, run from source
//! (`java -cp silicon.jar SiliconWarm.java`), so it needs no build step and
//! always matches the jar. A timeout or a crash ends the JVM; the next run
//! starts a new one and warms it up again.
//!
//! As in ViperHelium (`bench/src/silicon_warm.rs`), which this follows.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::LazyLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use regex::Regex;
use sea_orm::ActiveValue::Set;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, PaginatorTrait, QueryFilter};
use serde::Serialize;
use serde_json::json;

use super::helium::{BenchRun, run_status};
use super::{Executor, Measurement, MemberResult, Outcome, ParseError, RawRun, RunContext, Verdict};
use crate::db::silicon_runs;
use crate::explorer::Benchmark;
use crate::suite_config::SiliconConfig;
use crate::utils::{hash_file, sha256};

/// The driver's source, written next to the scratch files when it starts.
pub const DRIVER: &str = include_str!("../../silicon/SiliconWarm.java");

/// What the driver says first, and what starts the last line of every answer.
const END: &str = "@@END ";

/// Loading Silicon and compiling the driver take a few seconds.
const START_TIMEOUT: Duration = Duration::from_secs(180);

pub struct Silicon {
    java: PathBuf,
    jar: PathBuf,
    jvm_args: Vec<String>,
    args: Vec<String>,
    warmup_s: f64,
    file_timeout_s: u64,
    /// The warm-up files, benchmarks left out.
    files: Vec<PathBuf>,
    /// Warm-up files left out because a benchmark has the same content.
    excluded: usize,
    /// `sha256:<jar hash>|<config>|warm:<warm-up>`, see [`Silicon::new`].
    id: String,
    /// The version line Silicon prints (`Silicon 1.1 (abc@x)`), once a run
    /// has printed it.
    version: Option<String>,
    scratch: PathBuf,
    driver: Option<Driver>,
    /// Warm-up files each JVM started so far verified, in order.
    warmup_runs: Vec<usize>,
    /// Top-level declarations of the file last run, which [`Executor::parse`]
    /// attributes the errors to.
    decls: Vec<(u64, String)>,
}

impl Silicon {
    /// Collect the warm-up files, leaving out any with the same content as
    /// one of `benchmarks` (the `.vpr` files of every suite, activated or
    /// not, so the warm-up set does not change with the suites run). The JVM
    /// starts with the first run.
    ///
    /// The id covers everything that shapes a result besides the `.vpr`: the
    /// jar, the JVM and Silicon arguments (`-Xss` decides whether Silicon
    /// crashes, its flags what it reports), the driver, the warm-up files and
    /// the budget. Silicon's version line is not known before a run and is
    /// recorded in `meta`.
    pub fn new(
        java: PathBuf,
        config: &SiliconConfig,
        benchmarks: &[PathBuf],
        scratch: &Path,
    ) -> std::io::Result<Self> {
        let candidates = vpr_files(&config.warm.corpus);
        if candidates.is_empty() {
            return Err(std::io::Error::other(format!(
                "no .vpr files under the Silicon warm-up corpus {:?}",
                config.warm.corpus
            )));
        }
        // Only files of equal size can have equal content: hash just those.
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
        let sizes: BTreeSet<u64> = candidates.iter().filter_map(|p| size(p)).collect();
        let bench_shas: BTreeSet<String> = benchmarks
            .iter()
            .filter(|p| size(p).is_some_and(|s| sizes.contains(&s)))
            .filter_map(|p| hash_file(p).ok())
            .collect();
        let mut files = Vec::new();
        let mut shas = Vec::new();
        let mut excluded = 0;
        for p in candidates {
            let sha = hash_file(&p)?;
            if bench_shas.contains(&sha) {
                excluded += 1;
            } else {
                files.push(p);
                shas.push(sha);
            }
        }
        if files.is_empty() {
            return Err(std::io::Error::other("every Silicon warm-up file is also a benchmark"));
        }
        let config_id = sha256(format!("jvm {:?}\0silicon {:?}", config.jvm_args, config.args).as_bytes());
        let warm_id = sha256(
            format!(
                "{}\0{}\0{}\0{}",
                sha256(DRIVER.as_bytes()),
                shas.join(","),
                config.warm.warmup_s,
                config.warm.file_timeout_s
            )
            .as_bytes(),
        );
        let id = format!("sha256:{}|{}|warm:{}", hash_file(&config.jar)?, &config_id[..16], &warm_id[..16]);
        debug!("Silicon warm-up: {} files, {excluded} left out as benchmarks", files.len());
        Ok(Silicon {
            java,
            jar: config.jar.clone(),
            jvm_args: config.jvm_args.clone(),
            args: config.args.clone(),
            warmup_s: config.warm.warmup_s,
            file_timeout_s: config.warm.file_timeout_s,
            files,
            excluded,
            id,
            version: None,
            scratch: scratch.join("silicon-warm"),
            driver: None,
            warmup_runs: Vec::new(),
            decls: Vec::new(),
        })
    }

    /// A running, warmed-up driver.
    fn driver(&mut self) -> std::io::Result<&mut Driver> {
        if self.driver.is_none() {
            self.driver = Some(self.start()?);
        }
        Ok(self.driver.as_mut().unwrap())
    }

    fn start(&mut self) -> std::io::Result<Driver> {
        std::fs::create_dir_all(&self.scratch)?;
        let source = self.scratch.join("SiliconWarm.java");
        std::fs::write(&source, DRIVER)?;
        let list = self.scratch.join("warmup.txt");
        let listed: String = self.files.iter().map(|p| format!("{}\n", p.display())).collect();
        std::fs::write(&list, listed)?;
        let stderr = std::fs::File::create(self.scratch.join("stderr.txt"))?;

        let mut cmd = Command::new(&self.java);
        cmd.args(&self.jvm_args)
            .arg("-cp")
            .arg(&self.jar)
            .arg(&source)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr);
        debug!("starting the Silicon driver: {cmd:?}");
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut d = Driver { child, stdin, lines };
        // A driver that failed to start or warm up is ended with its Z3s.
        let fail = |d: &mut Driver, what: &str, r: Reply| {
            kill_tree(&mut d.child);
            std::io::Error::other(match r {
                Reply::TimedOut => format!("Silicon driver: no answer to {what}"),
                Reply::Died(out) | Reply::Done(out, _) => {
                    format!("Silicon driver: {what} failed: {}", tail(&out, 400))
                }
            })
        };
        let started = Instant::now();
        match d.reply(Some(START_TIMEOUT)) {
            Reply::Done(_, end) if end.first().is_some_and(|e| e == "READY") => {}
            r => return Err(fail(&mut d, "start", r)),
        }
        info!(
            "Silicon: JVM started in {:.1}s, warming it up for {}s on {} files",
            started.elapsed().as_secs_f64(),
            self.warmup_s,
            self.files.len()
        );
        if let Err(e) = d.send(&format!("WARM\t{}\t{}\t{}", self.warmup_s, self.file_timeout_s, list.display())) {
            kill_tree(&mut d.child);
            return Err(e);
        }
        // The budget is checked between files: allow one file past it, and
        // Silicon's own timeout is approximate.
        let limit = self.warmup_s + 4.0 * self.file_timeout_s as f64 + 120.0;
        match d.reply(Some(Duration::from_secs_f64(limit))) {
            Reply::Done(_, end) if end.first().is_some_and(|e| e == "WARM") => {
                let runs = end.get(1).and_then(|n| n.parse().ok()).unwrap_or(0);
                let secs: f64 = end.get(2).and_then(|s| s.parse().ok()).unwrap_or(f64::NAN);
                info!(
                    "Silicon: JVM {} warmed up in {secs:.1}s on {runs} files",
                    self.warmup_runs.len() + 1
                );
                self.warmup_runs.push(runs);
            }
            r => return Err(fail(&mut d, "warm-up", r)),
        }
        Ok(d)
    }

    /// End the JVM (and its Z3s).
    pub fn stop(&mut self) {
        if let Some(mut d) = self.driver.take() {
            let _ = d.send("QUIT");
            kill_tree(&mut d.child);
        }
    }
}

impl Drop for Silicon {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Driver {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

/// One answer from the driver.
enum Reply {
    /// The output before the `@@END` line, and that line's fields.
    Done(String, Vec<String>),
    TimedOut,
    /// The JVM exited (a crash, an out-of-memory error), with what it printed.
    Died(String),
}

impl Driver {
    fn send(&mut self, line: &str) -> std::io::Result<()> {
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()
    }

    /// `None`: wait as long as it takes.
    fn reply(&mut self, timeout: Option<Duration>) -> Reply {
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut out = String::new();
        loop {
            let line = match deadline {
                Some(d) => self.lines.recv_timeout(d.saturating_duration_since(Instant::now())),
                None => self.lines.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match line {
                Ok(line) => match line.strip_prefix(END) {
                    Some(end) => return Reply::Done(out, end.split(' ').map(String::from).collect()),
                    None => {
                        out.push_str(&line);
                        out.push('\n');
                    }
                },
                Err(RecvTimeoutError::Timeout) => return Reply::TimedOut,
                Err(RecvTimeoutError::Disconnected) => return Reply::Died(out),
            }
        }
    }
}

/// Kill `child` and everything it started: the JVM's Z3s would otherwise
/// keep running after a timeout.
fn kill_tree(child: &mut Child) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Every `.vpr` under `dirs`, in path order.
pub fn vpr_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "vpr") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    for d in dirs {
        walk(d, &mut out);
    }
    out.sort();
    out
}

/// The last `max` bytes of `s` (on a character boundary), trimmed.
fn tail(s: &str, max: usize) -> &str {
    let s = s.trim();
    let mut start = s.len().saturating_sub(max);
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

impl Executor for Silicon {
    fn name(&self) -> &'static str {
        "silicon"
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn input<'a>(&self, bench: &'a Benchmark) -> Option<&'a Path> {
        bench.viper.as_deref()
    }

    /// The JVM is warm from the corpus; repeating the file first would let
    /// the JIT specialise on it, which a cold run cannot.
    fn per_file_warmup(&self) -> bool {
        false
    }

    async fn check_existing<C: ConnectionTrait>(&self, conn: &C, sha256: &str) -> Result<usize, DbErr> {
        silicon_runs::Entity::find()
            .filter(silicon_runs::Column::BackendVersion.eq(&self.id))
            .filter(silicon_runs::Column::VprSha256.eq(sha256))
            .count(conn)
            .await
            .map(|n| n as usize)
    }

    /// One `RUN` in the warm JVM, started (and warmed up) first if none is
    /// running. On a timeout or a JVM exit the JVM is gone; the next run
    /// starts a new one.
    fn run(&mut self, input: &Path, ctx: &RunContext) -> std::io::Result<RawRun> {
        self.decls = declaration_lines(&std::fs::read_to_string(input)?);
        std::fs::create_dir_all(&ctx.scratch)?;
        let d = self.driver()?;
        d.send(&format!("RUN\t{}", input.display()))?;
        let raw = match d.reply(ctx.timeout) {
            Reply::Done(out, end) => {
                let ok = end.get(1).is_some_and(|s| s == "ok");
                let wall = end.get(2).and_then(|s| s.parse().ok()).unwrap_or(f64::NAN);
                RawRun {
                    wall,
                    peak_rss_mb: None,
                    exit_code: Some(if ok { 0 } else { 1 }),
                    // A run that threw printed its stack trace, which the
                    // crash message is taken from.
                    stderr: if ok { String::new() } else { out.clone() },
                    stdout: out,
                    timed_out: false,
                }
            }
            Reply::TimedOut => {
                warn!("Silicon: {} timed out, ending the JVM", input.display());
                self.stop();
                RawRun {
                    wall: ctx.timeout.map_or(f64::NAN, |t| t.as_secs_f64()),
                    peak_rss_mb: None,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    timed_out: true,
                }
            }
            Reply::Died(out) => {
                warn!("Silicon: the JVM exited on {}", input.display());
                self.stop();
                RawRun {
                    wall: f64::NAN,
                    peak_rss_mb: None,
                    exit_code: None,
                    stderr: format!("the Silicon JVM exited\n{}", tail(&out, 400)),
                    stdout: out,
                    timed_out: false,
                }
            }
        };
        std::fs::write(ctx.scratch.join("stdout.txt"), &raw.stdout)?;
        std::fs::write(ctx.scratch.join("stderr.txt"), &raw.stderr)?;
        if self.version.is_none() {
            self.version = VERSION.find(&raw.stdout).map(|m| m.as_str().to_string());
        }
        Ok(raw)
    }

    /// `members` are the members with at least one error, each `Failed` with
    /// its errors' messages; Silicon does not report the members that verify.
    fn parse(&self, raw: &RawRun) -> Result<Outcome, ParseError> {
        match raw.exit_code {
            Some(0) => {}
            Some(_) => return Err(ParseError("Silicon threw".into())),
            None => return Err(ParseError("the Silicon JVM exited".into())),
        }
        let out = parse_output(&raw.stdout, &self.decls);
        let verified = out
            .verified
            .ok_or_else(|| ParseError("no `Silicon finished/found` summary line".into()))?;
        let mut failed: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for e in out.errors {
            // An error outside every member (or one that could not be read)
            // still fails the file.
            let name = e.member.unwrap_or_else(|| match e.line {
                Some(l) => format!("line {l}"),
                None => "<unplaced>".into(),
            });
            failed.entry(name).or_default().push(e.message);
        }
        Ok(Outcome {
            verdict: if verified { Verdict::Verified } else { Verdict::Failed },
            self_time: out.verify_time,
            phases: BTreeMap::new(),
            members: failed
                .into_iter()
                .map(|(name, messages)| MemberResult {
                    name,
                    verdict: Verdict::Failed,
                    seconds: None,
                    message: Some(messages.join("\n")),
                })
                .collect(),
        })
    }

    /// Insert the timed runs of one [`super::measure`] of `bench` into
    /// `silicon_runs`, numbered on from the runs already stored. The
    /// benchmark's metrics row must exist already (foreign key on the hash).
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
        let read = |e: std::io::Error| DbErr::Custom(format!("cannot read {}: {e}", vpr.display()));
        let vpr_sha256 = hash_file(vpr).map_err(read)?;
        let decls = declaration_lines(&std::fs::read_to_string(vpr).map_err(read)?);
        let existing_runs = self.check_existing(conn, &vpr_sha256).await.unwrap_or(0);
        let rows = measurements.iter().enumerate().map(|(i, m)| {
            let out = parse_output(&m.runs.stdout, &decls);
            let finished = out.verified.is_some();
            let failed: BTreeSet<&str> = out.errors.iter().filter_map(|e| e.member.as_deref()).collect();
            let message = match &m.outcomes.verdict {
                Verdict::Crashed(msg) if !msg.is_empty() => Some(msg.clone()),
                _ => None,
            };
            silicon_runs::ActiveModel {
                run_id: Set(run.run_id.clone()),
                started_at: Set(run.started_at),
                host: Set(run.host.clone()),
                suite: Set(bench.suite.clone()),
                family: Set(bench.family.clone()),
                test: Set(bench.test.clone()),
                vpr_sha256: Set(vpr_sha256.clone()),
                backend_version: Set(Some(self.id.clone())),
                iteration: Set((existing_runs + i) as i32),
                status: Set(run_status(&m.outcomes.verdict)),
                timeout_s: Set(ctx.timeout.map(|t| t.as_secs_f64())),
                wall_s: Set(Some(m.runs.wall)),
                peak_rss_mb: Set(m.runs.peak_rss_mb),
                message: Set(message),
                meta: Set(Some(json!({
                    "version": out.version.as_ref().or(self.version.as_ref()),
                    "exit_code": m.runs.exit_code,
                    "jvm_args": self.jvm_args,
                    "args": self.args,
                    "warm": {
                        "warmup_files": self.files.len(),
                        "excluded": self.excluded,
                        "warmup_s": self.warmup_s,
                        "file_timeout_s": self.file_timeout_s,
                        "jvms": self.warmup_runs.len(),
                        "warmup_runs": self.warmup_runs,
                    },
                }))),
                verify_s: Set(m.outcomes.self_time),
                cached: Set(Some(false)),
                error_count: Set(finished.then_some(out.errors.len() as i32)),
                failed_members: Set(finished.then_some(failed.len() as i32)),
                errors: Set(finished.then(|| json!(out.errors))),
                ..Default::default()
            }
        });
        silicon_runs::Entity::insert_many(rows).exec(conn).await?;
        debug!("{bench}: stored {} Silicon runs from iteration {existing_runs}", measurements.len());
        Ok(())
    }
}

/// One Silicon error, attributed to the member it is in.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SiliconError {
    pub member: Option<String>,
    pub line: Option<u64>,
    pub message: String,
}

/// Parsed Silicon output.
#[derive(Debug, Clone, PartialEq)]
pub struct SiliconOutput {
    pub version: Option<String>,
    /// Whether the whole file verified; `None` without a summary line.
    pub verified: Option<bool>,
    /// Seconds Silicon reports for verification.
    pub verify_time: Option<f64>,
    pub errors: Vec<SiliconError>,
}

static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^Silicon \S+ \(.*\)").unwrap());

/// Silicon's summary line. Its time is `12.34s` under a minute, `01m:05s`
/// under an hour and `1h:02m:03s` above (silver's `formatMillisReadably`);
/// `ms` is accepted too.
static FINISHED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^Silicon (finished verification successfully|found (\d+) errors?) in (?:(\d+)h:)?(?:(\d+)m:)?([0-9.]+)\s*(m?s)",
    )
    .unwrap()
});

/// One error line: `  [0] <message> (<file>@<line>.<col>)`. The position is
/// often a range, `@320.11--321.30`; the error belongs to its start line.
/// From ten errors on, Silicon pads the index: `[ 0]` ... `[10]`.
static ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*\[\s*\d+\]\s+(.*?)\s*\(([^()@]*)@(\d+)\.(\d+)(?:--\d+\.\d+)?\)\s*$")
        .unwrap()
});

static DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(method|function|predicate|domain|field|adt|define|import)\s+([A-Za-z_$][\w$']*)")
        .unwrap()
});

/// Top-level declarations of a Viper source with their first line (1-based),
/// in order. A line belongs to the last declaration that starts at or before
/// it.
pub fn declaration_lines(source: &str) -> Vec<(u64, String)> {
    source
        .lines()
        .enumerate()
        .filter_map(|(i, l)| DECL.captures(l).map(|c| (i as u64 + 1, c[2].to_string())))
        .collect()
}

fn member_at(decls: &[(u64, String)], line: u64) -> Option<String> {
    decls.iter().rev().find(|(l, _)| *l <= line).map(|(_, n)| n.clone())
}

pub fn parse_output(out: &str, decls: &[(u64, String)]) -> SiliconOutput {
    let finished = FINISHED.captures(out);
    let verify_time = finished.as_ref().and_then(|c| {
        let part = |i: usize| c.get(i).map_or(Some(0.0), |m| m.as_str().parse::<f64>().ok());
        let x: f64 = c[5].parse().ok()?;
        let secs = if &c[6] == "ms" { x / 1000.0 } else { x };
        Some(part(3)? * 3600.0 + part(4)? * 60.0 + secs)
    });
    let verified = finished.as_ref().map(|c| c[1].starts_with("finished"));
    let mut errors: Vec<SiliconError> = ERROR
        .captures_iter(out)
        .map(|c| {
            let line = c[3].parse().ok();
            SiliconError {
                member: line.and_then(|l| member_at(decls, l)),
                line,
                message: c[1].to_string(),
            }
        })
        .collect();
    // Errors Silicon counted but we could not read stand in as one error
    // outside every member, so no member is taken to verify: a dropped error
    // would otherwise hide a soundness disagreement.
    let reported = finished
        .as_ref()
        .and_then(|c| c.get(2))
        .and_then(|n| n.as_str().parse::<usize>().ok());
    if let Some(n) = reported.filter(|&n| n > errors.len()) {
        errors.push(SiliconError {
            member: None,
            line: None,
            message: format!("{} of Silicon's {n} errors could not be read", n - errors.len()),
        });
    }
    SiliconOutput {
        version: VERSION.find(out).map(|m| m.as_str().to_string()),
        verified,
        verify_time,
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "field f: Int\n\nmethod m_a(x: Ref)\n{\n  assert false\n}\n\nmethod m_b()\n{\n}\n\nfunction g(): Int\n{ 1 }\n";

    #[test]
    fn finds_declarations() {
        let d = declaration_lines(SRC);
        assert_eq!(d, [(1, "f".into()), (3, "m_a".into()), (8, "m_b".into()), (12, "g".into())]);
        assert_eq!(member_at(&d, 5).as_deref(), Some("m_a"));
        assert_eq!(member_at(&d, 9).as_deref(), Some("m_b"));
    }

    #[test]
    fn parses_success_and_failure() {
        let d = declaration_lines(SRC);
        let ok = "Silicon 1.1-SNAPSHOT (4dfc6b5b@(detached))\nSilicon finished verification successfully in 2.87s.\n";
        let p = parse_output(ok, &d);
        assert_eq!(p.version.as_deref(), Some("Silicon 1.1-SNAPSHOT (4dfc6b5b@(detached))"));
        assert_eq!((p.verified, p.verify_time), (Some(true), Some(2.87)));
        assert!(p.errors.is_empty());

        for (line, secs) in [("in 870ms.", 0.87), ("in 01m:05s.", 65.0), ("in 1h:02m:03s.", 3723.0)] {
            let p = parse_output(&format!("Silicon finished verification successfully {line}\n"), &d);
            assert_eq!((p.verified, p.verify_time), (Some(true), Some(secs)), "{line}");
        }

        let bad = "Silicon found 1 error in 3.10s:\n  [0] Assert might fail. Assertion false might not hold. (x.vpr@5.3)\n";
        let p = parse_output(bad, &d);
        assert_eq!((p.verified, p.verify_time), (Some(false), Some(3.1)));
        assert_eq!(
            p.errors,
            [SiliconError {
                member: Some("m_a".into()),
                line: Some(5),
                message: "Assert might fail. Assertion false might not hold.".into()
            }]
        );
    }

    /// Silicon usually reports a range (`@9.5--10.12`), with parentheses in
    /// the message and Windows line endings; the error goes to the member
    /// holding the range's start line.
    #[test]
    fn parses_error_ranges() {
        let d = declaration_lines(SRC);
        let out = "Silicon found 1 error in 18.56s:\r\n  [0] Postcondition of m_b might not hold. There might be insufficient permission to access p(get(old(x))) (x.vpr@9.5--10.12)\r\n";
        let p = parse_output(out, &d);
        assert_eq!(p.verified, Some(false));
        assert_eq!(p.errors[0].member.as_deref(), Some("m_b"));
        assert_eq!(p.errors[0].line, Some(9));
        assert!(p.errors[0].message.ends_with("p(get(old(x)))"));
    }

    /// From ten errors on, Silicon pads the index (`[ 0]`); every error must
    /// still be read, or the members it names pass as verified.
    #[test]
    fn parses_padded_error_indices() {
        let d = declaration_lines(SRC);
        let mut out = "Silicon found 11 errors in 31.37s:\n".to_string();
        for i in 0..11 {
            let line = if i == 0 { 5 } else { 9 };
            out += &format!("  [{i:>2}] Assert might fail. (x.vpr@{line}.10--{line}.38)\n");
        }
        let p = parse_output(&out, &d);
        assert_eq!(p.errors.len(), 11);
        assert!(p.errors.iter().all(|e| e.member.is_some()));
    }

    /// Errors Silicon counts but we cannot read become one unplaced error,
    /// so no member is taken to verify.
    #[test]
    fn unread_errors_are_unplaced() {
        let d = declaration_lines(SRC);
        let out = "Silicon found 3 errors in 1.00s:\n  [0] Assert might fail. (x.vpr@5.3)\n  [1] Something new {x.vpr, line 9}\n";
        let p = parse_output(out, &d);
        assert_eq!(p.errors.len(), 2);
        assert_eq!(p.errors[1].member, None);
        assert_eq!(p.errors[1].message, "2 of Silicon's 3 errors could not be read");
    }

    fn silicon(decls: Vec<(u64, String)>) -> Silicon {
        Silicon {
            java: "java".into(),
            jar: "s.jar".into(),
            jvm_args: vec![],
            args: vec![],
            warmup_s: 60.0,
            file_timeout_s: 10,
            files: vec![],
            excluded: 0,
            id: "s".into(),
            version: None,
            scratch: "scratch".into(),
            driver: None,
            warmup_runs: vec![],
            decls,
        }
    }

    fn raw(exit_code: Option<i32>, stdout: &str) -> RawRun {
        RawRun {
            wall: 1.0,
            peak_rss_mb: None,
            exit_code,
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    /// Errors are grouped per member, the verdict follows the summary line,
    /// and a run without one (or one that threw) does not parse.
    #[test]
    fn outcome_groups_errors_by_member() {
        let s = silicon(declaration_lines(SRC));
        let out = "Silicon found 3 errors in 2.50s:\n  [0] A. (x.vpr@5.3)\n  [1] B. (x.vpr@5.9)\n  [2] C. (x.vpr@9.1)\n";
        let o = s.parse(&raw(Some(0), out)).unwrap();
        assert_eq!((o.verdict, o.self_time), (Verdict::Failed, Some(2.5)));
        assert_eq!(o.members.len(), 2);
        assert_eq!(o.members[0].name, "m_a");
        assert_eq!(o.members[0].message.as_deref(), Some("A.\nB."));

        let ok = s.parse(&raw(Some(0), "Silicon finished verification successfully in 1.00s.\n")).unwrap();
        assert_eq!((ok.verdict, ok.members.len()), (Verdict::Verified, 0));

        assert!(s.parse(&raw(Some(0), "Exception in thread main\n")).is_err());
        assert!(s.parse(&raw(Some(1), "Silicon finished verification successfully in 1.00s.\n")).is_err());
        assert!(s.parse(&raw(None, "")).is_err());
    }

    /// A warm-up file with a benchmark's content is left out, wherever it
    /// lives; the id follows the warm-up set and the budget.
    #[test]
    fn warmup_corpus_leaves_out_benchmarks() {
        use crate::suite_config::WarmConfig;
        let dir = std::env::temp_dir().join(format!("silicon-warm-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let corpus = dir.join("corpus");
        std::fs::create_dir_all(corpus.join("sub")).unwrap();
        std::fs::write(corpus.join("a.vpr"), "method a() {}").unwrap();
        std::fs::write(corpus.join("sub/b.vpr"), "method b() {}").unwrap();
        std::fs::write(corpus.join("notes.txt"), "method b() {}").unwrap();
        let jar = dir.join("s.jar");
        std::fs::write(&jar, "jar").unwrap();
        let bench = dir.join("bench.vpr");
        std::fs::write(&bench, "method b() {}").unwrap();
        let config = |warmup_s| SiliconConfig {
            jar: jar.clone(),
            jvm_args: vec![],
            args: vec![],
            warm: WarmConfig { corpus: vec![corpus.clone()], warmup_s, file_timeout_s: 10 },
        };
        let new = |warmup_s, benchmarks: &[PathBuf]| {
            Silicon::new("java".into(), &config(warmup_s), benchmarks, &dir)
        };

        let s = new(60.0, &[bench.clone()]).unwrap();
        assert_eq!(s.files, [corpus.join("a.vpr")]);
        assert_eq!(s.excluded, 1);
        assert!(s.id.starts_with("sha256:"));

        let all = new(60.0, &[]).unwrap();
        assert_eq!(all.files.len(), 2);
        assert_ne!(all.id, s.id);
        assert_ne!(new(120.0, &[bench.clone()]).unwrap().id, s.id);

        std::fs::write(corpus.join("a.vpr"), "method b() {}").unwrap();
        assert!(new(60.0, &[bench]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
