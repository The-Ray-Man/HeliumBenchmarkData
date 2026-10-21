//! Command line. `runner` without a subcommand is `runner bench`, so a plain
//! `cargo run` keeps measuring with `./suite.local.json`. Everything else is
//! in the config file.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::ci::parse_commit;

#[derive(Debug, Parser)]
#[command(about = "Benchmarks Helium, Silicon and rustc on the suites", args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Arguments of `bench` when no subcommand is given.
    #[command(flatten)]
    bench: BenchArgs,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Measure the configured suites and store the runs (the default).
    Bench(BenchArgs),
    /// Store the static metrics of every configured benchmark file, without
    /// running any tool.
    Index(IndexArgs),
    /// Set which commits CI keeps runs for, remove the runs of all others,
    /// and print the commits that still need a complete run, one per line.
    Sync(SyncArgs),
    /// Remove a closed PR's target and its runs.
    ClosePr(ClosePrArgs),
}

#[derive(Debug, Args)]
pub struct IndexArgs {
    /// Suite configuration file; only its suites are read.
    #[arg(long, default_value = "./suite.local.json")]
    pub config: PathBuf,
}

#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Suite configuration file; its `helium.crate` is fetched for the main
    /// commits from `helium.start_commit` on.
    #[arg(long, default_value = "./suite.local.json")]
    pub config: PathBuf,

    /// An open PR and its head commit, `<number>=<full hash>`; repeated per PR.
    #[arg(long = "pr", value_parser = parse_pr)]
    pub prs: Vec<(i32, String)>,
}

#[derive(Debug, Args)]
pub struct ClosePrArgs {
    /// Number of the closed PR.
    #[arg(long, value_parser = clap::value_parser!(i32).range(1..))]
    pub pr: i32,
}

impl SyncArgs {
    /// Fails if a PR is given twice.
    pub fn check_prs(&self) -> Result<(), String> {
        let mut numbers: Vec<i32> = self.prs.iter().map(|(n, _)| *n).collect();
        numbers.sort_unstable();
        match numbers.windows(2).find(|w| w[0] == w[1]) {
            Some(w) => Err(format!("PR #{} given twice", w[0])),
            None => Ok(()),
        }
    }
}

fn parse_pr(s: &str) -> Result<(i32, String), String> {
    let (number, commit) = s.split_once('=').ok_or("expected `<number>=<full hash>`")?;
    let number: i32 = number.parse().ok().filter(|n| *n > 0).ok_or("expected a positive PR number")?;
    Ok((number, parse_commit(commit)?))
}

impl Cli {
    pub fn command(self) -> Command {
        self.command.unwrap_or(Command::Bench(self.bench))
    }
}

#[derive(Debug, Args)]
pub struct BenchArgs {
    /// Suite configuration file.
    #[arg(long, default_value = "./suite.local.json")]
    pub config: PathBuf,

    /// CI mode: check out and build `--helium-commit` in `helium.crate`, and
    /// record the run in `bench_runs`.
    #[arg(long, requires_all = ["run_id", "helium_commit"])]
    pub ci: bool,

    /// Id of the run, e.g. `gh-<run id>-<attempt>`; must be new.
    #[arg(long, requires = "ci")]
    pub run_id: Option<String>,

    /// Full hash of the Helium commit to measure.
    #[arg(long, requires = "ci", value_parser = parse_commit)]
    pub helium_commit: Option<String>,

    /// Link to the GitHub Actions run, stored with the run.
    #[arg(long, requires = "ci")]
    pub github_run_url: Option<String>,

    /// Number of the PR whose head `--helium-commit` is; without it, the
    /// commit is a main commit. Decides which target keeps the run.
    #[arg(long, requires = "ci", value_parser = clap::value_parser!(i32).range(1..))]
    pub pr: Option<i32>,
}

/// The arguments of CI mode.
pub struct CiArgs<'a> {
    pub run_id: &'a str,
    pub helium_commit: &'a str,
    pub github_run_url: Option<&'a str>,
    pub pr: Option<i32>,
}

impl BenchArgs {
    /// `Some` in CI mode (clap makes `--run-id` and `--helium-commit` required then).
    pub fn ci(&self) -> Option<CiArgs<'_>> {
        self.ci.then(|| CiArgs {
            run_id: self.run_id.as_deref().expect("--run-id"),
            helium_commit: self.helium_commit.as_deref().expect("--helium-commit"),
            github_run_url: self.github_run_url.as_deref(),
            pr: self.pr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bench(args: &[&str]) -> BenchArgs {
        match Cli::try_parse_from(args).unwrap().command() {
            Command::Bench(b) => b,
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn parses_config() {
        assert_eq!(bench(&["runner"]).config, PathBuf::from("./suite.local.json"));
        assert_eq!(bench(&["runner", "bench", "--config", "suite.ci.json"]).config, PathBuf::from("suite.ci.json"));
        // Without the subcommand it means the same.
        assert_eq!(bench(&["runner", "--config", "suite.ci.json"]).config, PathBuf::from("suite.ci.json"));
        assert!(Cli::try_parse_from(["runner", "--tools", "helium"]).is_err());
        assert!(bench(&["runner"]).ci().is_none());
    }

    #[test]
    fn parses_index_sync_and_close_pr() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let pr = format!("12={b}");
        match Cli::try_parse_from(["runner", "sync", "--pr", &pr, "--pr", &format!("3={a}")]).unwrap().command() {
            Command::Sync(s) => {
                assert_eq!(s.config, PathBuf::from("./suite.local.json"));
                assert_eq!(s.prs, [(12, b.clone()), (3, a.clone())]);
            }
            c => panic!("{c:?}"),
        }
        match Cli::try_parse_from(["runner", "index", "--config", "suite.ci.json"]).unwrap().command() {
            Command::Index(i) => assert_eq!(i.config, PathBuf::from("suite.ci.json")),
            c => panic!("{c:?}"),
        }
        match Cli::try_parse_from(["runner", "close-pr", "--pr", "12"]).unwrap().command() {
            Command::ClosePr(c) => assert_eq!(c.pr, 12),
            c => panic!("{c:?}"),
        }
        assert!(Cli::try_parse_from(["runner", "sync", "--main", "main.txt"]).is_err());
        assert!(Cli::try_parse_from(["runner", "close-pr", "--pr", "0"]).is_err());
        assert!(parse_pr(&format!("x={a}")).is_err());
        assert!(parse_pr("12=abc").is_err());
        assert!(parse_pr(&a).is_err());
    }

    #[test]
    fn rejects_a_pr_given_twice() {
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        let sync = |prs| SyncArgs { config: "unused".into(), prs };
        assert!(sync(vec![(1, a.clone()), (2, b.clone())]).check_prs().is_ok());
        assert!(sync(vec![(1, a), (1, b)]).check_prs().unwrap_err().contains("given twice"));
    }

    #[test]
    fn parses_ci_mode() {
        let commit = "b81d12363bed9fae54810183b94e353736205456";
        let b = bench(&["runner", "bench", "--ci", "--run-id", "gh-1-1", "--helium-commit", commit]);
        let ci = b.ci().unwrap();
        assert_eq!((ci.run_id, ci.helium_commit, ci.github_run_url, ci.pr), ("gh-1-1", commit, None, None));

        let b = bench(&["runner", "bench", "--ci", "--run-id", "gh-1-1", "--helium-commit", commit, "--pr", "12"]);
        assert_eq!(b.ci().unwrap().pr, Some(12));

        // --ci needs both, and the others need --ci.
        assert!(Cli::try_parse_from(["runner", "--ci", "--run-id", "x"]).is_err());
        assert!(Cli::try_parse_from(["runner", "--ci", "--helium-commit", commit]).is_err());
        assert!(Cli::try_parse_from(["runner", "--run-id", "x", "--helium-commit", commit]).is_err());
        assert!(Cli::try_parse_from(["runner", "--pr", "12"]).is_err());
        assert!(Cli::try_parse_from(["runner", "--ci", "--run-id", "x", "--helium-commit", "b81d123"]).is_err());
        assert!(Cli::try_parse_from(["runner", "--ci", "--run-id", "x", "--helium-commit", commit, "--pr", "0"]).is_err());
    }
}
