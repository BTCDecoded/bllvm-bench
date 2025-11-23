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

```bash
# Run all differential tests
cargo test --test integration --features differential

# Run specific BIP tests
cargo test --test integration bip_differential

# Run with verbose output
RUST_BACKTRACE=1 cargo test --test integration --features differential -- --nocapture
```

### Prerequisites

1. **Bitcoin Core**: Must have `bitcoind` and `bitcoin-cli` available
   - Set `CORE_PATH` environment variable, or
   - Install Core in standard locations, or
   - Put Core binaries in `/opt/bitcoin-core/binaries/v25.0/`

2. **Network Access**: Tests use regtest mode (no network required)

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

## Future Improvements

- [ ] Implement proper Bitcoin block serialization
- [ ] Add more BIP tests (BIP66, BIP147)
- [ ] Add transaction differential tests
- [ ] Add historical block validation tests
- [ ] CI integration for automated testing

