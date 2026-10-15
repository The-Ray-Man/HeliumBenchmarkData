use crate::explorer::Explorer;
use crate::file_analyzer::record_benchmark;
use crate::suite_config::SuiteConfig;
use log::{error, info, warn};
mod db;
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
    env_logger::init();
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
    let mut counter = 0;
    for test_case in Explorer::new(&config) {
        match test_case {
            Ok(test_case) => {
                if let Err(err) = record_benchmark(&con, test_case.clone()).await {
                    error!("failed to insert bench {test_case:?}. err: {err}");
                } else {
                    counter += 1;
                }
            }
            Err(error) => error!("{error}"),
        }

        if (counter % 1000 == 0) {
            warn!("checked {} tests for being up to date with the database", counter);
        }
    }
}
