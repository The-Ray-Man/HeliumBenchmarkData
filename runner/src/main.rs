use crate::executor::helium::{BenchRun, Helium};
use crate::executor::rustc::Rustc;
use crate::executor::silicon::{self, Silicon};
use crate::executor::{measure, Executor, RunContext};
use crate::explorer::Explorer;
use crate::file_analyzer::record_benchmark;
use crate::suite_config::SuiteConfig;
use log::{debug, error, info};
mod db;
mod display;
mod explorer;
mod file_analyzer;
mod suite_config;
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
    // `block_on` runs `run` on this thread, so the async code gets its stack.
    std::thread::Builder::new()
        .name("runner".into())
        .stack_size(STACK_SIZE)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("a tokio runtime")
                .block_on(run())
        })
        .expect("spawning the runner thread")
        .join()
        .expect("the runner thread panicked");
}

async fn run() {
    let config = SuiteConfig::new("./suite.local.json").expect("a valid config. See config.example.json");
    let con = db::connection::connect().await.expect("could not connect to database");
    info!("Setting up Helium");
    let mut helium = Helium::new(config.helium.clone(), config.rustc_toolchain.as_deref()).expect("building Helium's `verify`");
    info!("Helium ready: {}", helium.id());

    info!("Setting up rustc");
    let mut rustc = Rustc::new(config.rustc_toolchain.as_deref(), vec![]).expect("finding rustc");
    info!("rustc ready: {}", rustc.id());

    let scratch = std::env::temp_dir().join("helium-bench");
    info!("Setting up Silicon");
    // Every suite's `.vpr` files, so none of them is in the warm-up set.
    let benchmarks = silicon::vpr_files(std::slice::from_ref(&config.suites_dir));
    let mut silicon = Silicon::new(config.java.clone(), &config.silicon, &benchmarks, &scratch)
        .expect("collecting Silicon's warm-up files");
    info!("Silicon ready: {}", silicon.id());

    let bench_run = BenchRun::new(&helium);
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

                match measure(&con, &mut helium, &test_case, &ctx).await {
                    Ok(measurements) => {
                        stats.runs += measurements.len();
                        if let Err(err) = helium.store(&con, &bench_run, &test_case, &ctx, &measurements).await {
                            error!("{test_case}: storing Helium runs failed: {err}");
                            stats.errors += 1;
                        }
                    }
                    Err(err) => {
                        error!("{test_case}: measuring Helium failed: {err}");
                        stats.errors += 1;
                    }
                }

                match measure(&con, &mut rustc, &test_case, &ctx).await {
                    Ok(measurements) => {
                        stats.runs += measurements.len();
                        if let Err(err) = rustc.store(&con, &bench_run, &test_case, &ctx, &measurements).await {
                            error!("{test_case}: storing rustc runs failed: {err}");
                            stats.errors += 1;
                        }
                    }
                    Err(err) => {
                        error!("{test_case}: measuring rustc failed: {err}");
                        stats.errors += 1;
                    }
                }

                match measure(&con, &mut silicon, &test_case, &ctx).await {
                    Ok(measurements) => {
                        stats.runs += measurements.len();
                        if let Err(err) = silicon.store(&con, &bench_run, &test_case, &ctx, &measurements).await {
                            error!("{test_case}: storing Silicon runs failed: {err}");
                            stats.errors += 1;
                        }
                    }
                    Err(err) => {
                        error!("{test_case}: measuring Silicon failed: {err}");
                        stats.errors += 1;
                    }
                }

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
}

/// Counters for the progress and summary lines.
#[derive(Default)]
struct RunStats {
    benchmarks: usize,
    /// Timed runs stored, over all tools.
    runs: usize,
    errors: usize,
}
