//! Differential Testing Framework
//!
//! This module provides functionality to compare BLLVM validation results
//! against Bitcoin Core's validation results.

use crate::core_rpc_client::CoreRpcClient;
use anyhow::{Context, Result};
use blvm_consensus::types::Network;
use blvm_consensus::{Block, Transaction};

/// Comparison result
#[derive(Debug, Clone)]
pub struct ComparisonResult {
    /// Whether results match
    pub matches: bool,
    /// BLLVM result
    pub bllvm_result: ValidationResult,
    /// Core result
    pub core_result: CoreValidationResult,
    /// Divergence details (if any)
    pub divergence: Option<DivergenceDetails>,
}

/// Validation result from BLLVM
#[derive(Debug, Clone)]
pub enum ValidationResult {
    Valid,
    Invalid(String),
}

/// Validation result from Core
#[derive(Debug, Clone)]
pub enum CoreValidationResult {
    Valid,
    Invalid(String),
}

/// Divergence details
#[derive(Debug, Clone)]
pub struct DivergenceDetails {
    /// Description of divergence
    pub description: String,
    /// BLLVM's reason
    pub bllvm_reason: Option<String>,
    /// Core's reason
    pub core_reason: Option<String>,
}

/// Compare transaction validation between BLLVM and Core
pub async fn compare_transaction_validation(
    tx: &Transaction,
    bllvm_result: ValidationResult,
    core_client: &CoreRpcClient,
) -> Result<ComparisonResult> {
    // Serialize transaction to hex using Bitcoin wire format
    // Use blvm-consensus serialization for proper format
    use blvm_consensus::serialization::serialize_transaction;
    let tx_bytes = serialize_transaction(tx);
    let tx_hex = hex::encode(tx_bytes);

    // Test with Core
    let core_result = core_client
        .testmempoolaccept(&tx_hex)
        .await
        .context("Failed to call testmempoolaccept")?;

    let core_validation = if core_result.allowed {
        CoreValidationResult::Valid
    } else {
        CoreValidationResult::Invalid(
            core_result
                .reject_reason
                .unwrap_or_else(|| "Unknown reason".to_string()),
        )
    };

    // Compare results
    let matches = match (&bllvm_result, &core_validation) {
        (ValidationResult::Valid, CoreValidationResult::Valid) => true,
        (ValidationResult::Invalid(_), CoreValidationResult::Invalid(_)) => true,
        _ => false,
    };

    let divergence = if !matches {
        Some(DivergenceDetails {
            description: "Transaction validation divergence between BLLVM and Core".to_string(),
            bllvm_reason: match &bllvm_result {
                ValidationResult::Invalid(reason) => Some(reason.clone()),
                _ => None,
            },
            core_reason: match &core_validation {
                CoreValidationResult::Invalid(reason) => Some(reason.clone()),
                _ => None,
            },
        })
    } else {
        None
    };

    Ok(ComparisonResult {
        matches,
        bllvm_result,
        core_result: core_validation,
        divergence,
    })
}

/// Compare block validation between BLLVM and Core
pub async fn compare_block_validation(
    block: &Block,
    height: u64,
    network: Network,
    bllvm_result: ValidationResult,
    core_client: &CoreRpcClient,
) -> Result<ComparisonResult> {
    // Serialize block to hex using Bitcoin wire format
    // Use the proper serialization from bllvm-consensus
    use blvm_consensus::serialization::block::serialize_block_header;
    use blvm_consensus::serialization::transaction::serialize_transaction;
    use blvm_consensus::serialization::varint::encode_varint;

    let mut block_bytes = Vec::new();
    // Serialize header (80 bytes)
    block_bytes.extend_from_slice(&serialize_block_header(&block.header));
    // Serialize transaction count (varint)
    let tx_count = block.transactions.len() as u64;
    let varint_bytes = encode_varint(tx_count);
    block_bytes.extend_from_slice(&varint_bytes);
    // Serialize transactions
    for tx in &block.transactions {
        block_bytes.extend_from_slice(&serialize_transaction(tx));
    }
    let block_hex = hex::encode(block_bytes);

    // Submit to Core
    let core_result = core_client
        .submitblock(&block_hex)
        .await
        .context("Failed to call submitblock")?;

    let core_validation = if core_result.accepted {
        CoreValidationResult::Valid
    } else {
        CoreValidationResult::Invalid(
            core_result
                .error
                .unwrap_or_else(|| "Unknown error".to_string()),
        )
    };

    // Compare results
    let matches = match (&bllvm_result, &core_validation) {
        (ValidationResult::Valid, CoreValidationResult::Valid) => true,
        (ValidationResult::Invalid(_), CoreValidationResult::Invalid(_)) => true,
        _ => false,
    };

    let divergence = if !matches {
        Some(DivergenceDetails {
            description: format!(
                "Block validation divergence at height {} between BLLVM and Core",
                height
            ),
            bllvm_reason: match &bllvm_result {
                ValidationResult::Invalid(reason) => Some(reason.clone()),
                _ => None,
            },
            core_reason: match &core_validation {
                CoreValidationResult::Invalid(reason) => Some(reason.clone()),
                _ => None,
            },
        })
    } else {
        None
    };

    Ok(ComparisonResult {
        matches,
        bllvm_result,
        core_result: core_validation,
        divergence,
    })
}

/// Format comparison result for display
pub fn format_comparison_result(result: &ComparisonResult) -> String {
    if result.matches {
        format!(
            "✅ MATCH: Both implementations agree ({:?})",
            result.bllvm_result
        )
    } else {
        let mut msg = format!("❌ DIVERGENCE: BLLVM and Core disagree\n");
        if let Some(ref div) = result.divergence {
            msg.push_str(&format!("  Description: {}\n", div.description));
            if let Some(ref reason) = div.bllvm_reason {
                msg.push_str(&format!("  BLLVM: {}\n", reason));
            }
            if let Some(ref reason) = div.core_reason {
                msg.push_str(&format!("  Core: {}\n", reason));
            }
        }
        msg
    }
}
