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
    }
}
