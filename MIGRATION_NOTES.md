# Benchmark Migration Notes

## Overview

All benchmarking code has been consolidated into the `bllvm-bench` crate. This is a development-only crate that supports testing in both development and production modes.

## What Was Moved

### From `bllvm-consensus/benches/`:
- `hash_operations.rs`
- `block_validation.rs`
- `block_validation_realistic.rs`
- `mempool_operations.rs`
- `segwit_operations.rs`
- `transaction_validation.rs`
- `utxo_commitments.rs`
- `bllvm_optimizations.rs`
- `performance_focused.rs`

### From `bllvm-node/benches/`:
- `compact_blocks.rs`
- `dandelion_bench.rs`
- `storage_operations.rs`
- `transport_comparison.rs`

## Changes Made

1. **Updated imports**: Changed `reference_node` → `bllvm_node`
2. **Fixed type mismatches**: Updated to use `tx_inputs!` and `tx_outputs!` macros for SmallVec compatibility
3. **Fixed Block structure**: Updated `transactions` field to use `Box<[Transaction]>`
4. **Enabled features**: Added `dandelion` and `utxo-commitments` features where needed

## Usage

### Running Rust Benchmarks

```bash
# Development mode (default)
cargo bench

# Production mode
cargo bench --features production

# Specific benchmark
cargo bench --bench hash_operations

# Using the CLI tool
cargo run --bin bllvm-bench -- rust
cargo run --bin bllvm-bench -- rust --production
cargo run --bin bllvm-bench -- rust hash_operations
```

### Running Shell Benchmarks

```bash
# Using the CLI tool
cargo run --bin bllvm-bench -- shell --all
cargo run --bin bllvm-bench -- shell run-all-fair-fast-benchmarks
```

### Running All Benchmarks

```bash
cargo run --bin bllvm-bench -- all
cargo run --bin bllvm-bench -- all --production
```

## Notes

- The original `benches/` directories in `bllvm-consensus` and `bllvm-node` still exist but are no longer used
- Bench definitions have been removed from both `Cargo.toml` files
- All benchmarks compile successfully in `bllvm-bench`

