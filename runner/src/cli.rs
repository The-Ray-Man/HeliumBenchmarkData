//! Command line. `runner` without a subcommand is `runner bench`, so a plain
//! `cargo run` keeps measuring with `./suite.local.json`. Everything else is
//! in the config file.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

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
    #[arg(long, requires = "ci", value_parser = crate::ci::parse_commit)]
    pub helium_commit: Option<String>,

    /// Link to the GitHub Actions run, stored with the run.
    #[arg(long, requires = "ci")]
    pub github_run_url: Option<String>,
}

/// The arguments of CI mode.
pub struct CiArgs<'a> {
    pub run_id: &'a str,
    pub helium_commit: &'a str,
    pub github_run_url: Option<&'a str>,
}

impl BenchArgs {
    /// `Some` in CI mode (clap makes `--run-id` and `--helium-commit` required then).
    pub fn ci(&self) -> Option<CiArgs<'_>> {
        self.ci.then(|| CiArgs {
            run_id: self.run_id.as_deref().expect("--run-id"),
            helium_commit: self.helium_commit.as_deref().expect("--helium-commit"),
            github_run_url: self.github_run_url.as_deref(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bench(args: &[&str]) -> BenchArgs {
        match Cli::try_parse_from(args).unwrap().command() {
            Command::Bench(b) => b,
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
    fn parses_ci_mode() {
        let commit = "b81d12363bed9fae54810183b94e353736205456";
        let b = bench(&["runner", "bench", "--ci", "--run-id", "gh-1-1", "--helium-commit", commit]);
        let ci = b.ci().unwrap();
        assert_eq!((ci.run_id, ci.helium_commit, ci.github_run_url), ("gh-1-1", commit, None));

        // --ci needs both, and the others need --ci.
        assert!(Cli::try_parse_from(["runner", "--ci", "--run-id", "x"]).is_err());
        assert!(Cli::try_parse_from(["runner", "--ci", "--helium-commit", commit]).is_err());
        assert!(Cli::try_parse_from(["runner", "--run-id", "x", "--helium-commit", commit]).is_err());
        assert!(Cli::try_parse_from(["runner", "--ci", "--run-id", "x", "--helium-commit", "b81d123"]).is_err());
    }
}
