use crate::cli::{BenchArgs, Cli, ClosePrArgs, Command, SyncArgs};
use crate::db::sea_orm_active_enums::BenchRunStatus;
use crate::executor::helium::{BenchRun, Helium};
use crate::executor::rustc::Rustc;
use crate::executor::silicon::{self, Silicon};
use crate::executor::{measure, Executor, RunContext};
use crate::explorer::{Benchmark, Explorer};
use crate::file_analyzer::record_benchmark;
use crate::suite_config::SuiteConfig;
use clap::Parser;
use log::{debug, error, info};
use sea_orm::ConnectionTrait;
mod ci;
mod cli;
mod db;
mod display;
mod explorer;
mod file_analyzer;
mod suite_config;
mod targets;
pub mod utils;
pub mod executor;

/// Stack of the thread that does the work. The Viper parser and the AST walk
/// recurse once per nesting level, and the largest suite files nest a few
/// hundred levels deep (cratesTop100/rustls.vpr: 381), which overflows the
/// 1 MB main thread on Windows. 64 MB is enough in a debug build; this leaves
/// headroom. Only touched pages are committed, so the size costs no memory.
const STACK_SIZE: usize = 256 * 1024 * 1024;

fn main() {
    // `info` unless RUST_LOG says otherwise (e.g. `RUST_LOG=runner=debug`).
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let command = Cli::parse().command();
    // `block_on` runs the command on this thread, so the async code gets its stack.
    let result = std::thread::Builder::new()
        .name("runner".into())
        .stack_size(STACK_SIZE)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("a tokio runtime")
                .block_on(async {
                    match command {
                        Command::Bench(args) => bench(args).await,
                        Command::Sync(args) => sync(args).await,
                        Command::ClosePr(args) => close_pr(args).await,
                    }
                })
        })
        .expect("spawning the runner thread")
        .join();
    if result.is_err() {
        // The panic message is already printed; record the crash.
        ci::mark_failed();
        std::process::exit(101);
    }
}

async fn sync(args: SyncArgs) {
    args.check_prs().unwrap_or_else(|e| panic!("{e}"));
    let config = SuiteConfig::new(&args.config).unwrap_or_else(|e| panic!("{e}. See suite.example.json"));
    let main = targets::main_commits(&config.helium.crate_dir, &config.helium.start_commit)
        .expect("listing the main commits");
    let desired = targets::Desired { main, prs: args.prs };
    let con = db::connection::connect().await.expect("could not connect to database");
    let missing = targets::sync(&con, &desired).await.expect("syncing the targets");
    // The log is on stderr, so stdout is just the list.
    for commit in missing {
        println!("{commit}");
    }
}

async fn close_pr(args: ClosePrArgs) {
    let con = db::connection::connect().await.expect("could not connect to database");
    targets::close_pr(&con, args.pr).await.expect("closing the PR");
}

async fn bench(args: BenchArgs) {
    let config = SuiteConfig::new(&args.config).unwrap_or_else(|e| panic!("{e}. See suite.example.json"));
    let con = db::connection::connect().await.expect("could not connect to database");
    let toolchain = config.rustc_toolchain.as_deref();

    // In CI mode the run is recorded before anything else, so a crash from
    // here on marks it FAILED.
    let ci_run = args.ci().map(|ci| (BenchRun::starting_now(ci.run_id), ci));
    if let Some((run, ci)) = &ci_run {
        ci::start(&con, run, ci.helium_commit, ci.github_run_url).await.expect("recording the CI run");
        ci::watch_cancel(con.clone());
    }

    info!("Setting up Helium");
    let mut helium = match &ci_run {
        Some((_, ci)) => Helium::at_commit(&config.helium, toolchain, ci.helium_commit),
        None => Helium::new(&config.helium, toolchain),
    }
    .expect("building Helium's `verify`");
    info!("Helium ready: {}", helium.id());

    info!("Setting up rustc");
    let mut rustc = Rustc::new(toolchain, vec![]).expect("finding rustc");
    info!("rustc ready: {}", rustc.id());

    let scratch = std::env::temp_dir().join("helium-bench");
    info!("Setting up Silicon");
    // Every suite's `.vpr` files, so none of them is in the warm-up set.
    let benchmarks = silicon::vpr_files(std::slice::from_ref(&config.suites_dir));
    let mut silicon = Silicon::new(config.java.clone(), &config.silicon, &benchmarks, &scratch)
        .expect("collecting Silicon's warm-up files");
    info!("Silicon ready: {}", silicon.id());

    let bench_run = match ci_run {
        Some((run, _)) => run,
        None => BenchRun::new(&helium),
    };
    let ctx = RunContext {
        scratch,
        timeout: config.timeout(),
        warmup: config.warmup_runs,
        runs: config.repeat_runs,
        force_rerun: false,
    };
    info!("Benchmark run {bench_run}");
    info!("Measuring with {ctx}");

    let mut stats = RunStats::default();
    for test_case in Explorer::new(&config) {
        match test_case {
            Ok(test_case) => {
                stats.benchmarks += 1;
                debug!("benchmark {test_case}");
                if let Err(err) = record_benchmark(&con, test_case.clone()).await {
                    error!("{test_case}: recording metrics failed: {err}");
                    stats.errors += 1;
                    continue;
                }

                run_tool(&con, &mut helium, "Helium", &bench_run, &test_case, &ctx, &mut stats).await;
                run_tool(&con, &mut rustc, "rustc", &bench_run, &test_case, &ctx, &mut stats).await;
                run_tool(&con, &mut silicon, "Silicon", &bench_run, &test_case, &ctx, &mut stats).await;

                if stats.benchmarks % 1000 == 0 {
                    info!("progress: {stats}");
                }
            }
            Err(error) => {
                error!("{error}");
                stats.errors += 1;
            }
        }
    }
    info!("Benchmark run {} finished: {stats}", bench_run.run_id);
    // Errors on single benchmarks do not fail the run: they are in its rows.
    if args.ci {
        ci::finish(&con, BenchRunStatus::Complete).await.expect("recording the CI run as complete");
    }
}

/// Measures `tool` on `test_case` and stores the runs; failures are logged
/// and counted.
async fn run_tool<C: ConnectionTrait, E: Executor>(
    con: &C,
    tool: &mut E,
    label: &str,
    bench_run: &BenchRun,
    test_case: &Benchmark,
    ctx: &RunContext,
    stats: &mut RunStats,
) {
    match measure(con, tool, test_case, ctx).await {
        Ok(measurements) => {
            stats.runs += measurements.len();
            if let Err(err) = tool.store(con, bench_run, test_case, ctx, &measurements).await {
                error!("{test_case}: storing {label} runs failed: {err}");
                stats.errors += 1;
            }
        }
        Err(err) => {
            error!("{test_case}: measuring {label} failed: {err}");
            stats.errors += 1;
        }
    }
}

/// Counters for the progress and summary lines.
#[derive(Default)]
struct RunStats {
    benchmarks: usize,
    /// Timed runs stored, over all tools.
    runs: usize,
    errors: usize,
}
