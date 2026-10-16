//! Connecting to the results database (the Postgres of compose.yaml).

use log::info;
use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr};

/// Used when `DATABASE_URL` is not set; matches the defaults in compose.yaml.
pub const DEFAULT_DATABASE_URL: &str = "postgres://helium:helium@localhost:5432/helium_bench";

/// The connection URL: `DATABASE_URL`, or [`DEFAULT_DATABASE_URL`].
pub fn database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.into())
}

/// Connects to [`database_url`].
pub async fn connect() -> Result<DatabaseConnection, DbErr> {
    info!("Connecting to database");
    connect_to(&database_url()).await
}

/// Connects to `url`. sqlx's log of every statement is off: at `info` it
/// would drown out the runner's own log.
pub async fn connect_to(url: &str) -> Result<DatabaseConnection, DbErr> {
    let mut options = ConnectOptions::new(url);
    options.sqlx_logging(false);
    Database::connect(options).await
}
