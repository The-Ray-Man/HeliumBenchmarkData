//! Static metrics of benchmark files, computed from the source alone.
//!
//! - [`rust`]: `.rs` files, the file-level columns of `rust_metrics`.
//! - [`viper`]: `.vpr` files, the text columns of `viper_metrics`; the rest
//!   (members, statements, ...) is reported by Helium's `verify --viper-metrics`.
//! - [`text`]: line counting shared by both.
//!
//! The metrics tables are keyed by the file's SHA-256 (`utils::sha256`).

pub mod rust;
pub mod text;
pub mod viper;
