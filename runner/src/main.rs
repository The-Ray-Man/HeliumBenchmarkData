use crate::explorer::Explorer;
use crate::suite_config::SuiteConfig;

mod db;
mod explorer;
mod suite_config;
pub mod utils;

fn main() {
    let config = SuiteConfig::new("./suite.local.json").expect("a valid config. See config.example.json");
    let explorer = Explorer::new(&config);

    explorer.into_iter().for_each(|test_case| {
        match test_case {
            Ok(test_case) => {
                println!("{:?}", test_case);
            },
            Err(error) => {
                println!("{}", error);
            }
        }
    });
}
