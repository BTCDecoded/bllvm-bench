//! blvm-bench - Development-only benchmarking suite for Bitcoin Commons BLVM
//!
//! This crate provides comprehensive benchmarking tools for the BLVM ecosystem.
//! While it's a development-only crate, it supports testing in production mode
//! to ensure benchmarks reflect real-world performance.

pub mod block_cache_env;

pub use block_cache_env::{
    bitcoin_data_dir_candidates, block_cache_dir_from_env,
    remote_core_ordered_blocks_cache_basename, remote_core_ordered_blocks_cache_basenames,
    remote_core_rpc_env_ready, remote_core_xor_blockfiles_hint, require_bitcoin_blk_dir,
    require_block_cache_dir, require_block_cache_subdir, sort_merge_data_dir,
};

pub mod deep_analysis;
/// Benchmark utilities and helpers
pub mod utils;

/// Shell benchmark runner
pub mod shell;

#[cfg(feature = "utxo-snapshot-tools")]
pub mod checkpoint_persistence;
/// Legacy module name; re-exports [`node_builder`](crate::node_builder).
#[cfg(any(feature = "differential", feature = "benchmark-helpers"))]
pub mod core_builder;
/// Legacy module name; re-exports [`node_rpc_client`](crate::node_rpc_client).
#[cfg(any(feature = "differential", feature = "benchmark-helpers"))]
pub mod core_rpc_client;
#[cfg(feature = "differential")]
pub mod differential;
/// Differential testing modules (feature-gated)
/// Also available for benchmarks via benchmark-helpers feature
#[cfg(any(feature = "differential", feature = "benchmark-helpers"))]
pub mod node_builder;
#[cfg(feature = "chunk-cache")]
pub mod node_rpc_client;
#[cfg(feature = "differential")]
pub mod parallel_differential;
#[cfg(any(feature = "differential", feature = "benchmark-helpers"))]
pub mod regtest_node;
#[cfg(feature = "utxo-snapshot-tools")]
pub mod utxo_delta;
#[cfg(any(feature = "utxo-snapshot-tools", feature = "disk-utxo"))]
pub mod utxo_snapshot_fixed_v1;
#[cfg(feature = "utxo-snapshot-tools")]
pub use checkpoint_persistence::CheckpointFormat;
#[cfg(feature = "differential")]
pub mod block_file_reader;
#[cfg(feature = "chunk-cache")]
pub mod chunk_index;
#[cfg(feature = "differential")]
pub mod chunk_index_rpc;
pub mod chunk_protection;
#[cfg(feature = "chunk-cache")]
pub mod chunked_cache;
#[cfg(feature = "differential")]
pub mod collect_only;
#[cfg(feature = "chunk-cache")]
pub mod missing_blocks;
pub mod remote_core_rpc;
// Archived: checkpoint_persistence - not used in sort-merge approach
// #[cfg(feature = "differential")]
// pub mod checkpoint_persistence;
// Archived: two_phase - abandoned in favor of sort-merge approach
// #[cfg(feature = "differential")]
// pub mod two_phase;
#[cfg(feature = "chunk-cache")]
pub mod chain_scan;
#[cfg(feature = "differential")]
pub mod script_validation;
#[cfg(feature = "differential")]
pub mod sort_merge;

#[cfg(feature = "bitcoinkernel")]
pub mod bitcoinkernel_ffi;
/// Default paths for `block_kernel_diff` (`~/.local/share/blvm-kernel-diff/`).
pub mod kernel_diff_paths;
#[cfg(any(feature = "bitcoinkernel", feature = "scan"))]
pub mod utxo_checkpoint_resolve;

#[cfg(feature = "disk-utxo")]
pub mod disk_utxo;

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
