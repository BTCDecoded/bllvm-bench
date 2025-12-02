//! Benchmark utilities and helpers

use std::path::PathBuf;

/// Get the path to the benchmarks directory
pub fn benchmarks_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("benchmarks")
}

/// Get the path to the results directory
pub fn results_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("results")
}

/// Check if production mode is enabled
pub fn is_production_mode() -> bool {
    cfg!(feature = "production")
}
