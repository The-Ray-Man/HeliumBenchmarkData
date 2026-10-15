//! Lookups of file metrics. Each returns the file's row, or `None` if the file
//! was never measured.

use std::path::Path;

use sea_orm::{ConnectionTrait, DbErr, EntityTrait};

use super::entities::{rust_metrics, viper_metrics};
use crate::utils;

/// Viper metrics of the file with SHA-256 `vpr_sha256` (lowercase hex).
pub async fn viper_metrics_by_sha256<C: ConnectionTrait>(
    db: &C,
    vpr_sha256: &str,
) -> Result<Option<viper_metrics::Model>, DbErr> {
    viper_metrics::Entity::find_by_id(vpr_sha256).one(db).await
}

/// Rust metrics of the file with SHA-256 `rs_sha256` (lowercase hex).
pub async fn rust_metrics_by_sha256<C: ConnectionTrait>(
    db: &C,
    rs_sha256: &str,
) -> Result<Option<rust_metrics::Model>, DbErr> {
    rust_metrics::Entity::find_by_id(rs_sha256).one(db).await
}

/// Metrics of the `.vpr` file at `path`, looked up by its content hash.
pub async fn viper_metrics_of_file<C: ConnectionTrait>(
    db: &C,
    path: impl AsRef<Path>,
) -> Result<Option<viper_metrics::Model>, DbErr> {
    viper_metrics_by_sha256(db, &hash_file(path.as_ref())?).await
}

/// Metrics of the `.rs` file at `path`, looked up by its content hash.
pub async fn rust_metrics_of_file<C: ConnectionTrait>(
    db: &C,
    path: impl AsRef<Path>,
) -> Result<Option<rust_metrics::Model>, DbErr> {
    rust_metrics_by_sha256(db, &hash_file(path.as_ref())?).await
}

fn hash_file(path: &Path) -> Result<String, DbErr> {
    utils::hash_file(path).map_err(|e| DbErr::Custom(format!("cannot read {}: {e}", path.display())))
}
