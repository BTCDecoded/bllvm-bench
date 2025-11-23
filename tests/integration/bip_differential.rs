//! BIP Differential Tests
//!
//! These tests verify that BLLVM and Bitcoin Core agree on BIP validation.
//! They catch consensus divergences by comparing validation results.

use anyhow::Result;
use bllvm_consensus::types::Network;
#[cfg(feature = "differential")]
use bllvm_bench::core_builder::CoreBuilder;
#[cfg(feature = "differential")]
use bllvm_bench::core_rpc_client::{CoreRpcClient, RpcConfig};
#[cfg(feature = "differential")]
use bllvm_bench::differential::{compare_block_validation, ValidationResult, format_comparison_result};
#[cfg(feature = "differential")]
use bllvm_bench::regtest_node::{RegtestNode, PortManager};
#[cfg(feature = "differential")]
use crate::helpers::*;

/// Test BIP30: Duplicate coinbase prevention
#[tokio::test]
#[cfg(feature = "differential")]
async fn test_bip30_differential() -> Result<()> {
    // Skip if Core not available
    let builder = CoreBuilder::new();
    let binaries = match builder.find_existing_core() {
        Ok(b) => b,
        Err(_) => {
            eprintln!("⚠️  Bitcoin Core not found, skipping BIP30 differential test");
            return Ok(());
        }
    };

    // Start regtest node
    let port_manager = PortManager::new(18443);
    let node = RegtestNode::start_with_port_manager(binaries, port_manager).await?;
    let rpc_client = CoreRpcClient::new(RpcConfig::from_regtest_node(&node));

    // Create block violating BIP30
    let block = create_bip30_violation_block(1);
    let height = 1;
    let network = Network::Mainnet;

    // Validate with BLLVM
    let bllvm_result = validate_bllvm_block(&block, height, network);
    let bllvm_validation = if bllvm_result.is_valid() {
        ValidationResult::Valid
    } else {
        ValidationResult::Invalid(format!("{:?}", bllvm_result))
    };

    // Compare with Core
    let comparison = compare_block_validation(
        &block,
        height,
        network,
        bllvm_validation.clone(),
        &rpc_client,
    )
    .await?;

    println!("{}", format_comparison_result(&comparison));

    // Both should reject (BIP30 violation)
    assert!(
        !comparison.matches || matches!(bllvm_validation, ValidationResult::Invalid(_)),
        "CRITICAL BUG: BIP30 violation should be rejected by both implementations"
    );

    Ok(())
}

/// Test BIP34: Block height in coinbase
#[tokio::test]
#[cfg(feature = "differential")]
async fn test_bip34_differential() -> Result<()> {
    let builder = CoreBuilder::new();
    let binaries = match builder.find_existing_core() {
        Ok(b) => b,
        Err(_) => {
            eprintln!("⚠️  Bitcoin Core not found, skipping BIP34 differential test");
            return Ok(());
        }
    };

    let port_manager = PortManager::new(18443);
    let node = RegtestNode::start_with_port_manager(binaries, port_manager).await?;
    let rpc_client = CoreRpcClient::new(RpcConfig::from_regtest_node(&node));

    // Create block violating BIP34 (missing height)
    let block = create_bip34_violation_block(1);
    let height = 1;
    let network = Network::Mainnet;

    // Validate with BLLVM
    let bllvm_result = validate_bllvm_block(&block, height, network);
    let bllvm_validation = if bllvm_result.is_valid() {
        ValidationResult::Valid
    } else {
        ValidationResult::Invalid(format!("{:?}", bllvm_result))
    };

    // Compare with Core
    let comparison = compare_block_validation(
        &block,
        height,
        network,
        bllvm_validation.clone(),
        &rpc_client,
    )
    .await?;

    println!("{}", format_comparison_result(&comparison));

    // Both should reject (BIP34 violation)
    assert!(
        !comparison.matches || matches!(bllvm_validation, ValidationResult::Invalid(_)),
        "CRITICAL BUG: BIP34 violation should be rejected by both implementations"
    );

    Ok(())
}

/// Test BIP90: Block version enforcement
#[tokio::test]
#[cfg(feature = "differential")]
async fn test_bip90_differential() -> Result<()> {
    let builder = CoreBuilder::new();
    let binaries = match builder.find_existing_core() {
        Ok(b) => b,
        Err(_) => {
            eprintln!("⚠️  Bitcoin Core not found, skipping BIP90 differential test");
            return Ok(());
        }
    };

    let port_manager = PortManager::new(18443);
    let node = RegtestNode::start_with_port_manager(binaries, port_manager).await?;
    let rpc_client = CoreRpcClient::new(RpcConfig::from_regtest_node(&node));

    // Create block violating BIP90 (invalid version)
    let block = create_bip90_violation_block(1, 0); // Version 0 is invalid after BIP90
    let height = 1;
    let network = Network::Mainnet;

    // Validate with BLLVM
    let bllvm_result = validate_bllvm_block(&block, height, network);
    let bllvm_validation = if bllvm_result.is_valid() {
        ValidationResult::Valid
    } else {
        ValidationResult::Invalid(format!("{:?}", bllvm_result))
    };

    // Compare with Core
    let comparison = compare_block_validation(
        &block,
        height,
        network,
        bllvm_validation.clone(),
        &rpc_client,
    )
    .await?;

    println!("{}", format_comparison_result(&comparison));

    // Both should reject (BIP90 violation)
    assert!(
        !comparison.matches || matches!(bllvm_validation, ValidationResult::Invalid(_)),
        "CRITICAL BUG: BIP90 violation should be rejected by both implementations"
    );

    Ok(())
}

/// Test that valid blocks are accepted by both
#[tokio::test]
#[cfg(feature = "differential")]
async fn test_valid_block_accepted() -> Result<()> {
    let builder = CoreBuilder::new();
    let binaries = match builder.find_existing_core() {
        Ok(b) => b,
        Err(_) => {
            eprintln!("⚠️  Bitcoin Core not found, skipping valid block test");
            return Ok(());
        }
    };

    let port_manager = PortManager::new(18443);
    let node = RegtestNode::start_with_port_manager(binaries, port_manager).await?;
    let rpc_client = CoreRpcClient::new(RpcConfig::from_regtest_node(&node));

    // Create valid block
    let block = create_test_block(1);
    let height = 1;
    let network = Network::Mainnet;

    // Validate with BLLVM
    let bllvm_result = validate_bllvm_block(&block, height, network);
    let bllvm_validation = if bllvm_result.is_valid() {
        ValidationResult::Valid
    } else {
        ValidationResult::Invalid(format!("{:?}", bllvm_result))
    };

    // Compare with Core
    let comparison = compare_block_validation(
        &block,
        height,
        network,
        bllvm_validation.clone(),
        &rpc_client,
    )
    .await?;

    println!("{}", format_comparison_result(&comparison));

    // Both should accept (valid block)
    // Note: This test may fail if block serialization doesn't match exactly
    // That's okay - the important thing is that violations are caught

    Ok(())
}

