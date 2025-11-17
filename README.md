# bllvm-bench

Development-only benchmarking suite for the Bitcoin Commons BLLVM ecosystem.

## Overview

This crate consolidates all benchmarking code for the BLLVM ecosystem. While it's a development-only crate, it supports testing in production mode to ensure benchmarks reflect real-world performance.

## Features

- **Rust Criterion Benchmarks**: Comprehensive performance benchmarks for all BLLVM components
- **Production Mode Testing**: Run benchmarks with production optimizations enabled
- **CLI Tool**: Command-line interface to run both Rust and shell-based benchmarks
- **Unified Results**: Consistent output format across all benchmark types

## Usage

### Running Rust Benchmarks

```bash
# Development mode (default)
cargo bench

# Production mode (with optimizations)
cargo bench --features production

# Specific benchmark
cargo bench --bench hash_operations
```

### Running Shell-Based Benchmarks

```bash
# Using the CLI tool
cargo run --bin bllvm-bench -- shell --all

# Specific benchmark suite
cargo run --bin bllvm-bench -- shell --suite rpc
```

### Running All Benchmarks

```bash
# Rust benchmarks in production mode
cargo bench --features production

# Shell benchmarks
cargo run --bin bllvm-bench -- shell --all
```

## Structure

```
bllvm-bench/
├── benches/              # Rust Criterion benchmarks
│   ├── consensus/        # Consensus layer benchmarks
│   ├── protocol/         # Protocol layer benchmarks
│   ├── node/             # Node implementation benchmarks
│   └── integration/      # Cross-component benchmarks
├── src/
│   ├── lib.rs            # Library code
│   ├── bin/
│   │   └── bllvm-bench.rs # CLI tool
│   └── shell/            # Shell benchmark runner
└── Cargo.toml
```

## Development vs Production Mode

- **Development Mode** (default): Faster iteration, less aggressive optimizations
- **Production Mode** (`--features production`): Full optimizations, reflects real-world performance

Both modes should be tested to ensure benchmarks are accurate in both scenarios.

## License

MIT

