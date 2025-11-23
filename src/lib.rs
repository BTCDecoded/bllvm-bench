//! bllvm-bench - Development-only benchmarking suite for Bitcoin Commons BLLVM
//!
//! This crate provides comprehensive benchmarking tools for the BLLVM ecosystem.
//! While it's a development-only crate, it supports testing in production mode
//! to ensure benchmarks reflect real-world performance.

/// Benchmark utilities and helpers
pub mod utils;
pub mod deep_analysis;

/// Shell benchmark runner
pub mod shell;

/// Differential testing modules (feature-gated)
#[cfg(feature = "differential")]
pub mod core_builder;
#[cfg(feature = "differential")]
pub mod regtest_node;
#[cfg(feature = "differential")]
pub mod core_rpc_client;
#[cfg(feature = "differential")]
pub mod differential;

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

