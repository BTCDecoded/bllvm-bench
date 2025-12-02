# Running Differential Tests Locally

## Quick Start

Yes, you can run the differential benchmarks locally! Here's how:

## Prerequisites

1. **Bitcoin Core binaries** (`bitcoind` and `bitcoin-cli`)
   - Must be built and available
   - Can be in PATH, or set `CORE_PATH` environment variable

2. **Rust toolchain** (already available)

## Setup Bitcoin Core

### Option 1: Use Existing Installation

If you have Bitcoin Core built somewhere:

```bash
export CORE_PATH=/path/to/bitcoin-core
```

The system will look for binaries in:
- `$CORE_PATH/build/bin/bitcoind` (CMake build)
- `$CORE_PATH/src/bitcoind` (autotools build)
- `$CORE_PATH/bin/bitcoind` (installed)

### Option 2: Build Bitcoin Core

```bash
# Clone Bitcoin Core
git clone https://github.com/bitcoin/bitcoin.git ~/bitcoin-core
cd ~/bitcoin-core

# Build with CMake (recommended)
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build -j$(nproc)

# Set environment variable
export CORE_PATH=~/bitcoin-core
```

### Option 3: Use System Installation

If `bitcoind` and `bitcoin-cli` are in your PATH:

```bash
# Just verify they're available
which bitcoind bitcoin-cli
```

## Running the Tests

### Run All Differential Tests

```bash
cd /home/acolyte/src/BitcoinCommons/bllvm-bench
cargo test --features differential
```

This will:
- Automatically discover Bitcoin Core binaries
- Start regtest nodes for each test
- Compare BLLVM vs Core validation results
- Skip gracefully if Core is not found

### Run Specific BIP Tests

```bash
# Test BIP30 (duplicate coinbase prevention)
cargo test --features differential test_bip30

# Test BIP34 (block height in coinbase)
cargo test --features differential test_bip34

# Test BIP90 (transaction replacement)
cargo test --features differential test_bip90
```

### Verbose Output

```bash
# See detailed comparison results
RUST_BACKTRACE=1 cargo test --features differential -- --nocapture

# Run specific test with verbose output
RUST_BACKTRACE=1 cargo test --features differential test_bip30 -- --nocapture
```

## How It Works

1. **Auto-Discovery**: The `CoreBuilder` automatically finds Bitcoin Core:
   - Checks `CORE_PATH` environment variable
   - Checks `BITCOIN_CORE_CACHE_DIR` for cached binaries
   - Searches common locations (`~/src/bitcoin`, `~/src/bitcoin-core`)
   - Checks if `bitcoind` is in PATH

2. **Regtest Nodes**: Each test starts an isolated Bitcoin Core regtest node:
   - Uses unique ports (18443-18543 range)
   - Creates temporary data directories
   - Automatically cleans up after tests

3. **Validation Comparison**: 
   - Creates test blocks with specific violations (BIP30, BIP34, etc.)
   - Validates with BLLVM
   - Validates with Bitcoin Core via RPC
   - Compares results to ensure consensus agreement

## Current Test Coverage

- **BIP30**: Duplicate coinbase transaction prevention
- **BIP34**: Block height in coinbase requirement
- **BIP90**: Transaction replacement (RBF) logic

## Troubleshooting

### "Bitcoin Core not found"

The tests will gracefully skip if Core is not found. To enable:

```bash
# Set CORE_PATH to your Core installation
export CORE_PATH=/path/to/bitcoin-core

# Or ensure bitcoind is in PATH
export PATH=/path/to/bitcoin-core/build/bin:$PATH
```

### Port Already in Use

Tests use ports 18443-18543. If you have other Bitcoin Core nodes running:

```bash
# Stop other regtest nodes
pkill bitcoind

# Or use a different port range (requires code modification)
```

### Test Failures

If tests fail, check:

1. **Core version**: Ensure you're using a compatible Core version (v25.0+)
2. **RPC access**: Tests need RPC access to Core (uses regtest, no network needed)
3. **Permissions**: Ensure Core binaries are executable

## Environment Variables

- `CORE_PATH`: Path to Bitcoin Core source/build directory
- `BITCOIN_CORE_CACHE_DIR`: Cache directory for Core binaries
- `KEEP_REGTEST_DATA`: Set to "1" to keep regtest data directories after tests (for debugging)

## Example Output

When tests run successfully, you'll see:

```
running 3 tests
test test_bip30_differential ... ok
test test_bip34_differential ... ok
test test_bip90_differential ... ok

test result: ok. 3 passed; 0 failed; 0 ignored
```

If Core is not found, tests will skip:

```
⚠️  Bitcoin Core not found, skipping BIP30 differential test
test test_bip30_differential ... ok (skipped)
```

## Next Steps

Once you have Core set up, you can:

1. Run the full test suite: `cargo test --features differential`
2. Add new BIP tests in `tests/integration/bip_differential.rs`
3. Extend the differential framework in `src/differential.rs`

