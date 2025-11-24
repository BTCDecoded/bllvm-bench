# Differential Testing in bllvm-bench

## Overview

Differential testing compares BLLVM's validation results against Bitcoin Core's validation results to catch consensus divergences.

## Features

- **Core Detection**: Automatically finds Bitcoin Core binaries
- **Regtest Node Management**: Starts/stops regtest nodes for testing
- **RPC Client**: Wrapper around Bitcoin Core RPC
- **Differential Framework**: Compare BLLVM vs Core validation
- **BIP Tests**: Specific tests for BIP30, BIP34, BIP90

## Usage

### Running Differential Tests

**Auto-Discovery (Recommended):** ✅
```bash
# Auto-discovers nodes from:
# 1. Environment variables
# 2. Bitcoin Core config files (~/.bitcoin/bitcoin.conf, etc.)
# 3. Common local configurations
# 4. Randomly selects from working nodes

cargo test --test integration --features differential
```

**Explicit Configuration:**
```bash
# Local Node (default)
cargo test --test integration --features differential

# Remote Node
export BITCOIN_RPC_HOST=your-node.example.com
export BITCOIN_RPC_PORT=8332
export BITCOIN_RPC_USER=rpcuser
export BITCOIN_RPC_PASSWORD=rpcpassword
export BITCOIN_NETWORK=mainnet
cargo test --test integration --features differential

# Disable Auto-Discovery (use explicit config only)
export BITCOIN_AUTO_DISCOVER=false
cargo test --test integration --features differential
```

**Specific Tests:**
```bash
# Run specific BIP tests
cargo test --test integration bip_differential

# Run with verbose output
RUST_BACKTRACE=1 cargo test --test integration --features differential -- --nocapture
```

### Prerequisites

1. **Bitcoin Core Node**: Must have access to a Bitcoin Core node (local or remote)
   
   **Option A: Local Node**
   - Must have `bitcoind` and `bitcoin-cli` available
   - Set `CORE_PATH` environment variable, or
   - Install Core in standard locations, or
   - Put Core binaries in `/opt/bitcoin-core/binaries/v25.0/`
   
   **Option B: Remote Node** ✅ **Supported!**
   - Set environment variables to connect to remote node:
     ```bash
     export BITCOIN_RPC_HOST=your-remote-host.com
     export BITCOIN_RPC_PORT=8332  # or 18332 for testnet, 18443 for regtest
     export BITCOIN_RPC_USER=your-rpc-user
     export BITCOIN_RPC_PASSWORD=your-rpc-password
     export BITCOIN_NETWORK=mainnet  # or testnet, regtest, signet
     ```
   - Remote node must have RPC enabled and accessible
   - Supports both pruned and unpruned remote nodes

2. **Network Access**: 
   - Local tests use regtest mode (no network required)
   - Remote tests require network access to the remote node

## Architecture

### Modules

- `core_builder.rs`: Finds/builds Bitcoin Core
- `regtest_node.rs`: Manages regtest nodes
- `core_rpc_client.rs`: RPC client wrapper
- `differential.rs`: Testing framework

### Test Structure

```
tests/integration/
├── mod.rs              # Test module declarations
├── helpers.rs          # Test helpers (block creation, etc.)
└── bip_differential.rs # BIP-specific differential tests
```

## How It Works

1. **Start Regtest Node**: Creates isolated Bitcoin Core regtest node
2. **Create Test Block**: Generates block with specific violation (e.g., BIP30)
3. **Validate with BLLVM**: Runs BLLVM validation
4. **Validate with Core**: Calls Core RPC to test validation
5. **Compare Results**: Ensures both implementations agree

## Example Test

```rust
#[tokio::test]
async fn test_bip30_differential() -> Result<()> {
    // Start regtest node
    let node = RegtestNode::start_with_port_manager(binaries, port_manager).await?;
    let rpc_client = CoreRpcClient::new(RpcConfig::from_regtest_node(&node));

    // Create block violating BIP30
    let block = create_bip30_violation_block(1);

    // Validate with BLLVM
    let bllvm_result = validate_bllvm_block(&block, 1, Network::Mainnet);

    // Compare with Core
    let comparison = compare_block_validation(
        &block, 1, Network::Mainnet, bllvm_result, &rpc_client
    ).await?;

    // Both should reject
    assert!(!comparison.matches || matches!(bllvm_result, ValidationResult::Invalid(_)));
    Ok(())
}
```

## Configuration

### Environment Variables

- `CORE_PATH`: Path to Bitcoin Core source/build directory
- `BITCOIN_CORE_CACHE_DIR`: Cache directory for Core binaries
- `KEEP_REGTEST_DATA`: Set to "1" to keep regtest data directories after tests

### Port Management

Tests use port manager to allocate unique ports (default: 18443-18543) for parallel test execution.

## Limitations

1. **Block Serialization**: Currently uses placeholder for block submission to Core
   - Validation comparison still works
   - Full block submission requires proper Bitcoin wire format

2. **Core Availability**: Tests gracefully skip if Core not found
   - Set `CORE_PATH` or install Core to enable tests

## Historical Block Testing

**Real differential testing** against actual Bitcoin blockchain blocks (0 to 800,000+).

### Running Historical Block Tests

```bash
# Test first 100 blocks (default)
cargo test --features differential test_historical_blocks_differential

# Test specific range (e.g., blocks 0-1000)
HISTORICAL_BLOCK_START=0 HISTORICAL_BLOCK_END=1000 \
  cargo test --features differential test_historical_blocks_differential

# Test up to block 800,000
HISTORICAL_BLOCK_START=0 HISTORICAL_BLOCK_END=800000 \
  cargo test --features differential test_historical_blocks_differential

# Test against remote node
BITCOIN_RPC_HOST=your-node.example.com \
BITCOIN_RPC_PORT=8332 \
BITCOIN_RPC_USER=rpcuser \
BITCOIN_RPC_PASSWORD=rpcpassword \
BITCOIN_NETWORK=mainnet \
HISTORICAL_BLOCK_START=0 HISTORICAL_BLOCK_END=1000 \
  cargo test --features differential test_historical_blocks_differential
```

### Prerequisites

1. **Mainnet Bitcoin Core Node**: Must have access to a synced mainnet node (local or remote)
   
   **Local Node:**
   - Set `BITCOIN_RPC_USER` and `BITCOIN_RPC_PASSWORD` if not using defaults
   - Node should be on port 8332 (default mainnet RPC port)
   - Or set `BITCOIN_RPC_PORT` to custom port
   
   **Remote Node:** ✅ **Supported!**
   ```bash
   export BITCOIN_RPC_HOST=your-remote-node.com
   export BITCOIN_RPC_PORT=8332
   export BITCOIN_RPC_USER=your-rpc-user
   export BITCOIN_RPC_PASSWORD=your-rpc-password
   export BITCOIN_NETWORK=mainnet
   ```
   - Remote node must be synced and have RPC enabled
   - Supports both pruned and unpruned remote nodes

2. **Pruned Nodes Supported**: ✅ **Yes, pruned nodes work!**
   - Automatically detects if node is pruned
   - Adjusts start height to match available blocks
   - Skips unavailable blocks gracefully
   - For pruned nodes, only tests blocks that are available (typically last ~550 blocks by default)
   - To test older blocks, use an unpruned (archival) node

3. **Block Availability**: 
   - **Unpruned node**: Can test any block from 0 to current height
   - **Pruned node**: Can only test blocks from `pruneheight` to current height

### How It Works

1. Connects to mainnet Core node (or falls back to regtest for local testing)
2. Iterates through blocks from `HISTORICAL_BLOCK_START` to `HISTORICAL_BLOCK_END`
3. Fetches each block from Core via RPC
4. Validates block with BLLVM (maintaining UTXO set state)
5. Compares BLLVM result with Core's validation
6. Reports any divergences

### Output

- Progress indicators every 1000 blocks
- Summary of tested/matched/diverged blocks
- Detailed divergence report (if any)
- Results recorded in differential test JSON

## Future Improvements

- [ ] Implement proper Bitcoin block serialization
- [ ] Add more BIP tests (BIP66, BIP147)
- [ ] Add transaction differential tests
- [x] Add historical block validation tests ✅
- [ ] CI integration for automated testing


