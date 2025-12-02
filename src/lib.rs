//! bllvm-bench - Development-only benchmarking suite for Bitcoin Commons BLLVM
//!
//! This crate provides comprehensive benchmarking tools for the BLLVM ecosystem.
//! While it's a development-only crate, it supports testing in production mode
//! to ensure benchmarks reflect real-world performance.

pub mod deep_analysis;
/// Benchmark utilities and helpers
pub mod utils;

/// Shell benchmark runner
pub mod shell;

/// Differential testing modules (feature-gated)
/// Also available for benchmarks via benchmark-helpers feature
#[cfg(any(feature = "differential", feature = "benchmark-helpers"))]
pub mod core_builder;
#[cfg(any(feature = "differential", feature = "benchmark-helpers"))]
pub mod core_rpc_client;
#[cfg(feature = "differential")]
pub mod differential;
#[cfg(any(feature = "differential", feature = "benchmark-helpers"))]
pub mod regtest_node;

use anyhow::Result;

/// Initialize benchmarking environment
pub fn init() -> Result<()> {
    // Set up any required environment variables or configuration
    Ok(())
}

/// Run all benchmarks
pub fn run_all() -> Result<()> {
    init()?;
    // This will be implemented to coordinate all benchmarks
    Ok(())
}
