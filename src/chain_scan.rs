//! Blockchain Scanner
//!
//! Scans blockchain blocks to measure:
//! 1. Output/witness size rule impact (e.g. BIP-110: output >34, OP_RETURN >83, witness >256)
//! 2. Transaction type breakdown (monetary vs Ordinals vs presigned money)
//! 3. Spam detection (SpamFilter) and cross-reference with rule-blocked txs
//!
//! Uses the same block iteration as differential testing (chunked cache).
//! Rule limits are configurable; defaults match BIP-110.

use blvm_protocol::opcodes::{
    OP_0, OP_ENDIF, OP_IF, OP_NOTIF, OP_PUSHDATA1, OP_PUSHDATA2, OP_PUSHDATA4, OP_RESERVED,
    OP_RETURN,
};
use blvm_protocol::segwit::Witness;
use blvm_protocol::spam_filter::{SpamFilter, SpamFilterResult, SpamType};
use blvm_protocol::witness;

/// Spam classification confidence (for debatable categories)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpamConfidence {
    /// Clear patterns: Ordinals, BRC-20
    Definite,
    /// LargeWitness, Dust, etc. - no Ordinals/BRC20
    Likely,
    /// LargeWitness only - could be Miniscript/vault
    Ambiguous,
    /// Not spam
    NotSpam,
}

fn spam_confidence(detected_types: &[SpamType]) -> SpamConfidence {
    if detected_types.is_empty() {
        return SpamConfidence::NotSpam;
    }
    let has_definite = detected_types
        .iter()
        .any(|t| matches!(t, SpamType::Ordinals | SpamType::BRC20));
    let has_large_witness = detected_types.iter().any(|t| *t == SpamType::LargeWitness);
    let has_other = detected_types.iter().any(|t| {
        matches!(
            t,
            SpamType::Dust
                | SpamType::ManySmallOutputs
                | SpamType::HighSizeValueRatio
                | SpamType::LowFeeRate
        )
    });

    if has_definite {
        SpamConfidence::Definite
    } else if has_other || (has_large_witness && detected_types.len() > 1) {
        SpamConfidence::Likely
    } else if has_large_witness {
        SpamConfidence::Ambiguous
    } else {
        SpamConfidence::Likely
    }
}
use blvm_protocol::block::calculate_tx_id;
use blvm_protocol::constants::{SEGWIT_ACTIVATION_MAINNET, TAPROOT_ACTIVATION_MAINNET};
use blvm_protocol::transaction::is_coinbase;
use blvm_protocol::types::{Block, OutPoint, Transaction};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Default limits (BIP-110)
const MAX_OUTPUT_SCRIPT_SIZE: usize = 34;
const MAX_OP_RETURN_SIZE: usize = 83;
const MAX_WITNESS_ELEMENT_SIZE: usize = 256;
const TAPROOT_CONTROL_MAX_SIZE: usize = 257;

/// Era boundaries for breakdown stats (mainnet consensus activation heights).
pub const SEGWIT_START_HEIGHT: u64 = SEGWIT_ACTIVATION_MAINNET;
pub const TAPROOT_START_HEIGHT: u64 = TAPROOT_ACTIVATION_MAINNET;
/// First block with Ordinals inscription (Dec 2022). Used for post-inscriptions era stats.
pub const INSCRIPTIONS_START_HEIGHT: u64 = 767_430;

fn block_era(height: u64) -> &'static str {
    if height >= INSCRIPTIONS_START_HEIGHT {
        "inscriptions"
    } else if height >= TAPROOT_START_HEIGHT {
        "taproot"
    } else if height >= SEGWIT_START_HEIGHT {
        "segwit"
    } else {
        "pre_segwit"
    }
}

fn witness_element_bucket(len: usize) -> &'static str {
    if len <= 256 {
        "0-256"
    } else if len <= 520 {
        "257-520"
    } else if len <= 1024 {
        "521-1024"
    } else {
        ">1024"
    }
}

/// Collateral witness element buckets (257-520, 521-999, 1000+)
fn collateral_witness_bucket(len: usize) -> &'static str {
    if len <= 520 {
        "257-520"
    } else if len <= 999 {
        "521-999"
    } else {
        "1000+"
    }
}

fn has_output_violation(violations: &[OutputSizeViolation]) -> bool {
    violations.iter().any(|v| {
        matches!(
            v,
            OutputSizeViolation::OutputScriptOversized(_)
                | OutputSizeViolation::OpReturnOversized(_)
        )
    })
}
fn has_witness_violation(violations: &[OutputSizeViolation]) -> bool {
    violations
        .iter()
        .any(|v| matches!(v, OutputSizeViolation::WitnessElementOversized(_)))
}
fn has_control_violation(violations: &[OutputSizeViolation]) -> bool {
    violations
        .iter()
        .any(|v| matches!(v, OutputSizeViolation::ControlBlockOversized(_)))
}
fn has_annex_violation(violations: &[OutputSizeViolation]) -> bool {
    violations
        .iter()
        .any(|v| matches!(v, OutputSizeViolation::AnnexPresent))
}

/// Compact size encoding length (1 byte for < 253)
fn compact_size_len(n: usize) -> u64 {
    if n < 0xfd {
        1
    } else if n <= 0xffff {
        3
    } else if n <= 0xffff_ffff {
        5
    } else {
        9
    }
}

/// Compute tx weight (BIP141: 4*base + total). Uses actual script sizes for accuracy.
fn tx_weight(tx: &Transaction, witnesses: &[Vec<Witness>], tx_idx: usize) -> u64 {
    let mut base_size = 4u64; // version
    for input in &tx.inputs {
        base_size += 32 + 4; // prevout
        base_size += compact_size_len(input.script_sig.len()) + input.script_sig.len() as u64;
        base_size += 4; // sequence
    }
    for output in &tx.outputs {
        base_size += 8; // value
        base_size +=
            compact_size_len(output.script_pubkey.len()) + output.script_pubkey.len() as u64;
    }
    base_size += 4; // locktime
    let witness_size: u64 = if tx_idx < witnesses.len() {
        witnesses[tx_idx]
            .iter()
            .flat_map(|w| w.iter())
            .map(|e| e.len() as u64)
            .sum()
    } else {
        0
    };
    let total_size = base_size + witness_size;
    witness::calculate_transaction_weight_segwit(base_size, total_size)
}

/// Output/witness size rule violation (e.g. BIP-110 limits)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum OutputSizeViolation {
    /// OP_RETURN output > 83 bytes
    OpReturnOversized(usize),
    /// Non-OP_RETURN output script > 34 bytes
    OutputScriptOversized(usize),
    /// Witness element > 256 bytes
    WitnessElementOversized(usize),
    /// Taproot control block > 257 bytes
    ControlBlockOversized(usize),
    /// Annex present (invalidated under BIP-110)
    AnnexPresent,
}

/// Transaction classification for type breakdown
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum TxClassification {
    /// Standard monetary (P2PKH, P2WPKH, P2TR key-path, etc.)
    Monetary,
    /// Ordinals/inscription patterns (envelope, large witness, Taproot script-path data)
    OrdinalsLike,
    /// BRC-20 patterns (OP_RETURN with JSON)
    Brc20Like,
    /// Complex script (Miniscript, vault) - could be presigned money
    ComplexScript,
    /// Mixed (has both monetary and data components)
    Mixed,
    /// Unknown / other
    Other,
}

/// Per-transaction scan result
#[derive(Debug, Clone, Serialize)]
pub struct TxScanResult {
    pub tx_idx: usize,
    pub would_block: bool,
    pub violations: Vec<OutputSizeViolation>,
    pub classification: TxClassification,
    pub max_witness_element: usize,
    pub total_witness_size: usize,
    pub has_envelope: bool,
    pub has_large_op_return: bool,
    /// Tx has Taproot script-path with Tapscript containing OP_IF/OP_NOTIF (BIP-110 forbids; any size)
    pub has_tapscript_op_if_violation: bool,
    /// Witness element sizes for histogram bucketing
    pub witness_element_sizes: Vec<usize>,
}

/// Era-specific stats (pre-segwit, segwit, taproot, inscriptions)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EraStats {
    pub blocks: u64,
    pub total_txs: u64,
    pub blocked_txs: u64,
    pub total_weight: u64,
    pub blocked_weight: u64,
}

/// Collateral breakdown by era (same four eras as era_*)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollateralByEra {
    pub pre_segwit: CollateralEraStats,
    pub segwit: CollateralEraStats,
    pub taproot: CollateralEraStats,
    pub inscriptions: CollateralEraStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollateralEraStats {
    pub collateral_txs: u64,
    pub collateral_weight: u64,
}

/// Per-retarget-period stats (2016 blocks)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetargetPeriodStats {
    pub start_height: u64,
    pub blocked_weight: u64,
    pub total_weight: u64,
    pub blocked_weight_pct: f64,
}

/// LargeWitness spam count by era
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LargeWitnessSpamByEra {
    pub pre_segwit: u64,
    pub segwit: u64,
    pub taproot: u64,
    pub inscriptions: u64,
}

/// Spam counts by type, per era (Ordinals, Dust, BRC20, LargeWitness, etc.)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpamByTypeByEra {
    pub pre_segwit: HashMap<String, u64>,
    pub segwit: HashMap<String, u64>,
    pub taproot: HashMap<String, u64>,
    pub inscriptions: HashMap<String, u64>,
}

/// Per-block aggregated stats
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockScanStats {
    pub height: u64,
    pub total_txs: usize,
    pub blocked_txs: usize,
    pub blocked_weight: u64,
    pub total_weight: u64,
    pub violations_by_type: HashMap<String, usize>,
    pub classifications: HashMap<String, usize>,
    pub txs_with_witness_element_gt_256: usize,
    pub txs_with_op_return_gt_83: usize,
    #[serde(default)]
    pub witness_element_histogram: HashMap<String, usize>,
    #[serde(default)]
    pub blocked_txs_with_output_violation: usize,
    #[serde(default)]
    pub blocked_txs_with_witness_violation: usize,
    #[serde(default)]
    pub blocked_txs_with_control_violation: usize,
    #[serde(default)]
    pub blocked_weight_with_output_violation: u64,
    #[serde(default)]
    pub blocked_weight_with_witness_violation: u64,
    #[serde(default)]
    pub blocked_weight_with_control_violation: u64,
    #[serde(default)]
    pub collateral_violations_by_type: HashMap<String, usize>,
    #[serde(default)]
    pub collateral_witness_element_histogram: HashMap<String, usize>,
    #[serde(default)]
    pub collateral_weight: u64,
    #[serde(default)]
    pub largwitness_spam: usize,
    #[serde(default)]
    pub largwitness_and_witness_blocked: usize,
    #[serde(default)]
    pub blocked_txs_with_taproot_output: usize,
    #[serde(default)]
    pub collateral_by_classification: HashMap<String, usize>,
    #[serde(default)]
    pub block_txs_with_tapscript_op_if_violation: usize,
    /// Tapscript OP_IF: prevout created before activation (can still spend)
    #[serde(default)]
    pub tapscript_op_if_grandfathered: usize,
    /// Tapscript OP_IF: prevout created at/after activation (would be stuck until BIP-110 expires)
    #[serde(default)]
    pub tapscript_op_if_unspendable: usize,
    // Spam cross-reference (when SpamFilter is used)
    #[serde(default)]
    pub spam_txs: usize,
    #[serde(default)]
    pub spam_by_type: HashMap<String, usize>,
    #[serde(default)]
    pub spam_and_rule_blocked: usize,
    #[serde(default)]
    pub spam_and_not_rule_blocked: usize,
    #[serde(default)]
    pub rule_blocked_and_not_spam: usize,
    #[serde(default)]
    pub spam_by_confidence: HashMap<String, usize>,
    #[serde(default)]
    pub spam_by_type_by_era: SpamByTypeByEra,
}

/// Global scan results
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChainScanResults {
    pub blocks_scanned: u64,
    pub total_txs: u64,
    pub blocked_txs: u64,
    pub blocked_weight: u64,
    pub total_weight: u64,
    /// Post-inscriptions era (block 767430+): blockspace impact is more concentrated
    #[serde(default)]
    pub blocks_post_inscriptions: u64,
    #[serde(default)]
    pub total_txs_post_inscriptions: u64,
    #[serde(default)]
    pub blocked_txs_post_inscriptions: u64,
    #[serde(default)]
    pub blocked_weight_post_inscriptions: u64,
    #[serde(default)]
    pub total_weight_post_inscriptions: u64,
    /// Era breakdown: pre_segwit, segwit, taproot, inscriptions
    #[serde(default)]
    pub era_pre_segwit: EraStats,
    #[serde(default)]
    pub era_segwit: EraStats,
    #[serde(default)]
    pub era_taproot: EraStats,
    #[serde(default)]
    pub era_inscriptions: EraStats,
    pub violations_by_type: HashMap<String, u64>,
    pub classifications: HashMap<String, u64>,
    pub witness_element_histogram: HashMap<String, u64>,
    #[serde(default)]
    pub blocked_txs_with_output_violation: u64,
    #[serde(default)]
    pub blocked_txs_with_witness_violation: u64,
    #[serde(default)]
    pub blocked_txs_with_control_violation: u64,
    #[serde(default)]
    pub blocked_weight_with_output_violation: u64,
    #[serde(default)]
    pub blocked_weight_with_witness_violation: u64,
    #[serde(default)]
    pub blocked_weight_with_control_violation: u64,
    #[serde(default)]
    pub collateral_violations_by_type: HashMap<String, u64>,
    #[serde(default)]
    pub collateral_witness_element_histogram: HashMap<String, u64>,
    #[serde(default)]
    pub collateral_by_era: CollateralByEra,
    #[serde(default)]
    pub blocked_weight_by_retarget: HashMap<u64, RetargetPeriodStats>,
    #[serde(default)]
    pub largwitness_spam_by_era: LargeWitnessSpamByEra,
    #[serde(default)]
    pub spam_by_type_by_era: SpamByTypeByEra,
    #[serde(default)]
    pub largwitness_and_witness_blocked: u64,
    #[serde(default)]
    pub blocked_txs_with_taproot_output: u64,
    #[serde(default)]
    pub collateral_by_classification: HashMap<String, u64>,
    #[serde(default)]
    pub block_txs_with_tapscript_op_if_violation: u64,
    #[serde(default)]
    pub tapscript_op_if_grandfathered: u64,
    #[serde(default)]
    pub tapscript_op_if_unspendable: u64,
    // Spam cross-reference (when SpamFilter is used)
    #[serde(default)]
    pub spam_txs: u64,
    #[serde(default)]
    pub spam_by_type: HashMap<String, u64>,
    #[serde(default)]
    pub spam_and_rule_blocked: u64,
    #[serde(default)]
    pub spam_and_not_rule_blocked: u64,
    #[serde(default)]
    pub rule_blocked_and_not_spam: u64,
    #[serde(default)]
    pub spam_by_confidence: HashMap<String, u64>,
}

/// Check if output would violate output size limits
fn check_output(script: &[u8]) -> Option<OutputSizeViolation> {
    if script.is_empty() {
        return None;
    }
    if script[0] == OP_RETURN {
        if script.len() > MAX_OP_RETURN_SIZE {
            return Some(OutputSizeViolation::OpReturnOversized(script.len()));
        }
    } else if script.len() > MAX_OUTPUT_SCRIPT_SIZE {
        return Some(OutputSizeViolation::OutputScriptOversized(script.len()));
    }
    None
}

/// Check witness for size limit violations
/// Returns (violations, max_element, total_size, element_sizes for histogram)
fn check_witness(witness: &Witness) -> (Vec<OutputSizeViolation>, usize, usize, Vec<usize>) {
    let mut violations = Vec::new();
    let mut max_element = 0usize;
    let mut total_size = 0usize;
    let mut element_sizes = Vec::with_capacity(witness.len());

    for element in witness {
        let len = element.len();
        element_sizes.push(len);
        total_size += len;
        if len > max_element {
            max_element = len;
        }
        if len > MAX_WITNESS_ELEMENT_SIZE {
            violations.push(OutputSizeViolation::WitnessElementOversized(len));
        }
    }

    // Check control block (last element in Taproot script-path spend)
    if witness.len() >= 2 {
        let last = &witness[witness.len() - 1];
        if last.len() >= 33 && (last.len() - 33) % 32 == 0 {
            // Looks like control block: 33 + 32n
            if last.len() > TAPROOT_CONTROL_MAX_SIZE {
                violations.push(OutputSizeViolation::ControlBlockOversized(last.len()));
            }
        }
    }

    (violations, max_element, total_size, element_sizes)
}

/// P2TR (Taproot) output: OP_1 (0x51) + 0x20 + 32-byte x-only pubkey = 34 bytes
fn is_taproot_output(script: &[u8]) -> bool {
    script.len() == 34 && script[0] == 0x51 && script[1] == 0x20
}

/// Transaction has at least one Taproot (P2TR) output
fn has_taproot_output(tx: &Transaction) -> bool {
    tx.outputs
        .iter()
        .any(|o| is_taproot_output(&o.script_pubkey))
}

/// Get the tapscript element from a Taproot script-path witness.
/// Structure: [stack..., script, annex?, control_block]. We must check only the script, not stack items
/// (signatures/hashes can contain 0x63/0x64 as data, causing false positives).
fn get_tapscript_element(witness: &Witness) -> Option<&[u8]> {
    if witness.len() < 2 {
        return None;
    }
    let last = witness.last().unwrap();
    if last.len() < 33 || (last.len() - 33) % 32 != 0 {
        return None;
    }
    let script_idx = if witness.len() >= 3 {
        let maybe_annex = &witness[witness.len() - 2];
        if maybe_annex.first() == Some(&OP_RESERVED) {
            witness.len() - 3
        } else {
            witness.len() - 2
        }
    } else {
        witness.len() - 2
    };
    Some(&witness[script_idx])
}

/// Walk script and return true if OP_IF or OP_NOTIF appears as an opcode (not inside push data).
/// Skips push opcodes: 0x01-0x4b (direct), OP_PUSHDATA1/2/4.
fn script_has_op_if_or_notif_as_opcode(script: &[u8]) -> bool {
    let mut i = 0;
    while i < script.len() {
        let opcode = script[i];
        let advance = if opcode > 0 && opcode < OP_PUSHDATA1 {
            // Direct push: opcode IS the length (1-75 bytes)
            let len = opcode as usize;
            if i + 1 + len > script.len() {
                return false; // Truncated
            }
            1 + len
        } else if opcode == OP_PUSHDATA1 {
            if i + 1 >= script.len() {
                return false;
            }
            let len = script[i + 1] as usize;
            if i + 2 + len > script.len() {
                return false;
            }
            2 + len
        } else if opcode == OP_PUSHDATA2 {
            if i + 2 >= script.len() {
                return false;
            }
            let len = u16::from_le_bytes([script[i + 1], script[i + 2]]) as usize;
            if i + 3 + len > script.len() {
                return false;
            }
            3 + len
        } else if opcode == OP_PUSHDATA4 {
            if i + 4 >= script.len() {
                return false;
            }
            let len =
                u32::from_le_bytes([script[i + 1], script[i + 2], script[i + 3], script[i + 4]])
                    as usize;
            if i + 5 + len > script.len() {
                return false;
            }
            5 + len
        } else {
            // Single-byte opcode
            if opcode == OP_IF || opcode == OP_NOTIF {
                return true;
            }
            1
        };
        i += advance;
    }
    false
}

/// Tapscript contains OP_IF or OP_NOTIF as opcodes (not as data in pushes).
/// Excludes Ordinals envelope (OP_0 OP_IF at start).
fn tapscript_contains_op_if_or_notif(script: &[u8]) -> bool {
    if script.len() >= 2 && script[0] == OP_0 && script[1] == OP_IF {
        return false;
    }
    script_has_op_if_or_notif_as_opcode(script)
}

/// Witness has Taproot script-path with tapscript (not stack items) containing OP_IF or OP_NOTIF.
/// Only checks the actual tapscript element to avoid false positives from signatures/hashes.
fn witness_has_tapscript_op_if(witness: &Witness) -> bool {
    get_tapscript_element(witness)
        .map(|script| tapscript_contains_op_if_or_notif(script))
        .unwrap_or(false)
}

/// Input indices that have Taproot script-path witness with OP_IF/OP_NOTIF in tapscript.
/// Used for grandfathered vs unspendable lookup (prevout creation height).
fn tapscript_op_if_input_indices(
    _tx: &Transaction,
    witnesses: &[Vec<Witness>],
    tx_idx: usize,
) -> Vec<usize> {
    let mut indices = Vec::new();
    if let Some(wits) = witnesses.get(tx_idx) {
        for (input_idx, w) in wits.iter().enumerate() {
            if witness_has_tapscript_op_if(w) {
                indices.push(input_idx);
            }
        }
    }
    indices
}

/// Check for annex (last witness element with OP_RESERVED prefix in Taproot)
fn has_annex(witness: &Witness) -> bool {
    if witness.len() < 2 {
        return false;
    }
    let last = &witness[witness.len() - 1];
    last.len() >= 1 && last[0] == OP_RESERVED
}

/// Envelope protocol: OP_FALSE OP_IF ... OP_ENDIF
fn has_envelope_pattern(script: &[u8]) -> bool {
    if script.len() < 4 {
        return false;
    }
    if script[0] == OP_0 && script[1] == OP_IF {
        if script.iter().skip(2).any(|&b| b == OP_ENDIF) {
            return true;
        }
    }
    false
}

/// BRC-20: OP_RETURN with JSON-like data
fn has_brc20_pattern(script: &[u8]) -> bool {
    if script.len() < 10 || script[0] != OP_RETURN {
        return false;
    }
    let data = &script[1..];
    if data.len() > 100 {
        return false;
    }
    let s = String::from_utf8_lossy(data);
    s.contains('{') && (s.contains("op") || s.contains("tick") || s.contains("amt"))
}

/// Classify transaction type
fn classify_tx(tx: &Transaction, witnesses: &[Vec<Witness>], tx_idx: usize) -> TxClassification {
    let tx_witnesses = witnesses.get(tx_idx);

    let mut has_ordinal_pattern = false;
    let mut has_brc20 = false;
    let mut has_complex_script = false;
    let mut has_monetary = false;

    for output in &tx.outputs {
        if output.script_pubkey.is_empty() {
            continue;
        }
        if has_envelope_pattern(&output.script_pubkey)
            || (output.script_pubkey.len() > 200 && output.script_pubkey[0] != OP_RETURN)
        {
            has_ordinal_pattern = true;
        }
        if has_brc20_pattern(&output.script_pubkey) {
            has_brc20 = true;
        }
        if output.script_pubkey.len() <= 34 && output.script_pubkey[0] != OP_RETURN {
            has_monetary = true;
        }
    }

    for input in &tx.inputs {
        if has_envelope_pattern(&input.script_sig) {
            has_ordinal_pattern = true;
        }
        if input.script_sig.len() > 300 {
            has_complex_script = true;
        }
    }

    if let Some(wits) = tx_witnesses {
        for w in wits {
            let total: usize = w.iter().map(|e| e.len()).sum();
            if total > 500 {
                has_ordinal_pattern = true;
            }
            for elem in w {
                if elem.len() > 300 {
                    has_complex_script = true;
                }
            }
        }
    }

    if has_brc20 {
        TxClassification::Brc20Like
    } else if has_ordinal_pattern && has_monetary {
        TxClassification::Mixed
    } else if has_ordinal_pattern {
        TxClassification::OrdinalsLike
    } else if has_complex_script {
        TxClassification::ComplexScript
    } else if has_monetary {
        TxClassification::Monetary
    } else {
        TxClassification::Other
    }
}

/// Analyze a single transaction for output/witness size rule violations
pub fn analyze_tx(tx: &Transaction, witnesses: &[Vec<Witness>], tx_idx: usize) -> TxScanResult {
    let mut violations = Vec::new();
    let mut max_witness_element = 0usize;
    let mut total_witness_size = 0usize;
    let mut has_envelope = false;
    let mut has_large_op_return = false;
    let mut has_tapscript_op_if_violation = false;
    let mut witness_element_sizes = Vec::new();

    for output in &tx.outputs {
        if let Some(v) = check_output(&output.script_pubkey) {
            violations.push(v.clone());
            if matches!(v, OutputSizeViolation::OpReturnOversized(_)) {
                has_large_op_return = true;
            }
        }
    }

    for input in &tx.inputs {
        if has_envelope_pattern(&input.script_sig) {
            has_envelope = true;
        }
    }

    if let Some(wits) = witnesses.get(tx_idx) {
        for w in wits {
            for elem in w {
                if elem.len() >= 4 && elem[0] == OP_0 && elem[1] == OP_IF {
                    has_envelope = true;
                    break;
                }
            }
            if witness_has_tapscript_op_if(w) {
                has_tapscript_op_if_violation = true;
            }
            let (v, max, total, sizes) = check_witness(w);
            violations.extend(v);
            witness_element_sizes.extend(sizes);
            if max > max_witness_element {
                max_witness_element = max;
            }
            total_witness_size += total;
        }
        if wits.iter().any(|w| has_annex(w)) {
            violations.push(OutputSizeViolation::AnnexPresent);
        }
    }

    let classification = classify_tx(tx, witnesses, tx_idx);

    let would_block = !violations.is_empty();

    TxScanResult {
        tx_idx,
        would_block,
        violations,
        classification,
        max_witness_element,
        total_witness_size,
        has_envelope,
        has_large_op_return,
        has_tapscript_op_if_violation,
        witness_element_sizes,
    }
}

/// Analyze a block and return aggregated stats.
/// If `spam_filter` is Some, runs SpamFilter and computes spam vs rule-blocked cross-reference.
/// Uses rayon to parallelize per-transaction work (analyze_tx, tx_weight, spam detection).
pub fn analyze_block(
    block: &Block,
    witnesses: &[Vec<Witness>],
    height: u64,
    spam_filter: Option<&SpamFilter>,
) -> BlockScanStats {
    let witnesses = witnesses; // capture for closure
    let spam_filter = spam_filter; // capture for closure

    // Parallel per-tx work: analyze, weight, spam detection
    let tx_results: Vec<(TxScanResult, u64, Option<SpamFilterResult>)> = block
        .transactions
        .par_iter()
        .enumerate()
        .map(|(tx_idx, tx)| {
            let result = analyze_tx(tx, witnesses, tx_idx);
            let w = tx_weight(tx, witnesses, tx_idx);
            let spam_result = spam_filter.map(|f| {
                let tx_wits = witnesses.get(tx_idx).map(|w| w.as_slice());
                f.is_spam_with_witness(tx, tx_wits, None)
            });
            (result, w, spam_result)
        })
        .collect();

    // Sequential aggregation (fast)
    let mut stats = BlockScanStats {
        height,
        total_txs: block.transactions.len(),
        ..Default::default()
    };
    let mut violations_by_type: HashMap<String, usize> = HashMap::new();
    let mut classifications: HashMap<String, usize> = HashMap::new();
    let mut spam_by_type: HashMap<String, usize> = HashMap::new();
    let mut spam_by_confidence: HashMap<String, usize> = HashMap::new();
    let mut witness_element_histogram: HashMap<String, usize> = HashMap::new();
    let mut collateral_violations_by_type: HashMap<String, usize> = HashMap::new();
    let mut collateral_witness_element_histogram: HashMap<String, usize> = HashMap::new();
    let mut collateral_weight: u64 = 0;
    let mut largwitness_spam: usize = 0;
    let mut largwitness_and_witness_blocked: usize = 0;
    let mut blocked_txs_with_taproot_output: usize = 0;
    let mut collateral_by_classification: HashMap<String, usize> = HashMap::new();
    let mut spam_by_type_by_era: SpamByTypeByEra = SpamByTypeByEra::default();

    for (i, (result, w, spam_result)) in tx_results.into_iter().enumerate() {
        let tx = &block.transactions[i];
        stats.total_weight += w;
        if result.would_block {
            stats.blocked_txs += 1;
            stats.blocked_weight += w;
            if has_taproot_output(tx) {
                blocked_txs_with_taproot_output += 1;
            }
            if has_output_violation(&result.violations) {
                stats.blocked_txs_with_output_violation += 1;
                stats.blocked_weight_with_output_violation += w;
            }
            if has_witness_violation(&result.violations) {
                stats.blocked_txs_with_witness_violation += 1;
                stats.blocked_weight_with_witness_violation += w;
            }
            if has_control_violation(&result.violations) {
                stats.blocked_txs_with_control_violation += 1;
                stats.blocked_weight_with_control_violation += w;
            }
        }
        // BIP-110 OP_IF rule: count ALL txs with Tapscript containing OP_IF/OP_NOTIF (any size)
        if result.has_tapscript_op_if_violation {
            stats.block_txs_with_tapscript_op_if_violation += 1;
        }

        let class_key = format!("{:?}", result.classification);
        *classifications.entry(class_key.clone()).or_insert(0) += 1;

        for v in &result.violations {
            let key = format!("{:?}", v);
            *violations_by_type.entry(key).or_insert(0) += 1;
        }

        for &sz in &result.witness_element_sizes {
            let bucket = witness_element_bucket(sz).to_string();
            *witness_element_histogram.entry(bucket).or_insert(0) += 1;
        }

        if result.max_witness_element > MAX_WITNESS_ELEMENT_SIZE {
            stats.txs_with_witness_element_gt_256 += 1;
        }
        if result.has_large_op_return {
            stats.txs_with_op_return_gt_83 += 1;
        }

        if let Some(sr) = spam_result {
            if sr.is_spam {
                stats.spam_txs += 1;
                let era = block_era(height);
                for st in &sr.detected_types {
                    if *st != SpamType::NotSpam {
                        let key = format!("{:?}", st);
                        *spam_by_type.entry(key.clone()).or_insert(0) += 1;
                        let era_map = match era {
                            "pre_segwit" => &mut spam_by_type_by_era.pre_segwit,
                            "segwit" => &mut spam_by_type_by_era.segwit,
                            "taproot" => &mut spam_by_type_by_era.taproot,
                            _ => &mut spam_by_type_by_era.inscriptions,
                        };
                        *era_map.entry(key).or_insert(0) += 1u64;
                    }
                }
                if sr.detected_types.contains(&SpamType::LargeWitness) {
                    largwitness_spam += 1;
                    if result.would_block && has_witness_violation(&result.violations) {
                        largwitness_and_witness_blocked += 1;
                    }
                }
                let confidence = spam_confidence(&sr.detected_types);
                *spam_by_confidence
                    .entry(format!("{:?}", confidence))
                    .or_insert(0) += 1;
                if result.would_block {
                    stats.spam_and_rule_blocked += 1;
                } else {
                    stats.spam_and_not_rule_blocked += 1;
                }
            } else if result.would_block {
                stats.rule_blocked_and_not_spam += 1;
                collateral_weight += w;
                *collateral_by_classification
                    .entry(class_key.clone())
                    .or_insert(0) += 1;
                for v in &result.violations {
                    let key = format!("{:?}", v);
                    *collateral_violations_by_type.entry(key).or_insert(0) += 1;
                    if let OutputSizeViolation::WitnessElementOversized(len) = v {
                        let bucket = collateral_witness_bucket(*len).to_string();
                        *collateral_witness_element_histogram
                            .entry(bucket)
                            .or_insert(0) += 1;
                    }
                }
            }
        }
    }

    stats.violations_by_type = violations_by_type;
    stats.classifications = classifications;
    stats.spam_by_type = spam_by_type;
    stats.spam_by_confidence = spam_by_confidence;
    stats.witness_element_histogram = witness_element_histogram;
    stats.collateral_violations_by_type = collateral_violations_by_type;
    stats.collateral_witness_element_histogram = collateral_witness_element_histogram;
    stats.collateral_weight = collateral_weight;
    stats.largwitness_spam = largwitness_spam;
    stats.largwitness_and_witness_blocked = largwitness_and_witness_blocked;
    stats.blocked_txs_with_taproot_output = blocked_txs_with_taproot_output;
    stats.collateral_by_classification = collateral_by_classification;
    stats.spam_by_type_by_era = spam_by_type_by_era;

    stats
}

/// Analyze a block with outpoint index for grandfathered vs unspendable classification.
/// Processes txs sequentially to maintain index. Use when --grandfathered is set.
/// Only stores P2TR outputs in the index (tapscript OP_IF only affects Taproot spends) to reduce memory.
pub fn analyze_block_with_outpoint_index(
    block: &Block,
    witnesses: &[Vec<Witness>],
    height: u64,
    spam_filter: Option<&SpamFilter>,
    outpoint_index: &mut FxHashMap<OutPoint, u32>,
    bip110_activation_height: u64,
) -> BlockScanStats {
    let mut stats = BlockScanStats {
        height,
        total_txs: block.transactions.len(),
        ..Default::default()
    };
    let mut violations_by_type: HashMap<String, usize> = HashMap::new();
    let mut classifications: HashMap<String, usize> = HashMap::new();
    let mut spam_by_type: HashMap<String, usize> = HashMap::new();
    let mut spam_by_confidence: HashMap<String, usize> = HashMap::new();
    let mut witness_element_histogram: HashMap<String, usize> = HashMap::new();
    let mut collateral_violations_by_type: HashMap<String, usize> = HashMap::new();
    let mut collateral_witness_element_histogram: HashMap<String, usize> = HashMap::new();
    let mut collateral_weight: u64 = 0;
    let mut largwitness_spam: usize = 0;
    let mut largwitness_and_witness_blocked: usize = 0;
    let mut blocked_txs_with_taproot_output: usize = 0;
    let mut collateral_by_classification: HashMap<String, usize> = HashMap::new();
    let mut spam_by_type_by_era: SpamByTypeByEra = SpamByTypeByEra::default();

    for (tx_idx, tx) in block.transactions.iter().enumerate() {
        let tx_id = calculate_tx_id(tx);
        // Only store P2TR outputs: tapscript OP_IF lookups only need prevouts that are Taproot spends.
        // This reduces index from ~150M entries to ~30M, avoiding OOM.
        for (vout, output) in tx.outputs.iter().enumerate() {
            if is_taproot_output(&output.script_pubkey) {
                outpoint_index.insert(
                    OutPoint {
                        hash: tx_id,
                        index: vout as u32,
                    },
                    height as u32,
                );
            }
        }

        let result = analyze_tx(tx, witnesses, tx_idx);
        let w = tx_weight(tx, witnesses, tx_idx);
        let spam_result = spam_filter.map(|f| {
            let tx_wits = witnesses.get(tx_idx).map(|w| w.as_slice());
            f.is_spam_with_witness(tx, tx_wits, None)
        });

        stats.total_weight += w;
        if result.would_block {
            stats.blocked_txs += 1;
            stats.blocked_weight += w;
            if has_taproot_output(tx) {
                blocked_txs_with_taproot_output += 1;
            }
            if has_output_violation(&result.violations) {
                stats.blocked_txs_with_output_violation += 1;
                stats.blocked_weight_with_output_violation += w;
            }
            if has_witness_violation(&result.violations) {
                stats.blocked_txs_with_witness_violation += 1;
                stats.blocked_weight_with_witness_violation += w;
            }
            if has_control_violation(&result.violations) {
                stats.blocked_txs_with_control_violation += 1;
                stats.blocked_weight_with_control_violation += w;
            }
        }
        if result.has_tapscript_op_if_violation {
            stats.block_txs_with_tapscript_op_if_violation += 1;
            for input_idx in tapscript_op_if_input_indices(tx, witnesses, tx_idx) {
                let prevout = tx.inputs[input_idx].prevout;
                if let Some(&creation_height) = outpoint_index.get(&prevout) {
                    if (creation_height as u64) < bip110_activation_height {
                        stats.tapscript_op_if_grandfathered += 1;
                    } else {
                        stats.tapscript_op_if_unspendable += 1;
                    }
                }
            }
        }

        let class_key = format!("{:?}", result.classification);
        *classifications.entry(class_key.clone()).or_insert(0) += 1;
        for v in &result.violations {
            let key = format!("{:?}", v);
            *violations_by_type.entry(key).or_insert(0) += 1;
        }
        for &sz in &result.witness_element_sizes {
            let bucket = witness_element_bucket(sz).to_string();
            *witness_element_histogram.entry(bucket).or_insert(0) += 1;
        }
        if result.max_witness_element > MAX_WITNESS_ELEMENT_SIZE {
            stats.txs_with_witness_element_gt_256 += 1;
        }
        if result.has_large_op_return {
            stats.txs_with_op_return_gt_83 += 1;
        }

        if let Some(sr) = spam_result {
            if sr.is_spam {
                stats.spam_txs += 1;
                let era = block_era(height);
                for st in &sr.detected_types {
                    if *st != SpamType::NotSpam {
                        let key = format!("{:?}", st);
                        *spam_by_type.entry(key.clone()).or_insert(0) += 1;
                        let era_map = match era {
                            "pre_segwit" => &mut spam_by_type_by_era.pre_segwit,
                            "segwit" => &mut spam_by_type_by_era.segwit,
                            "taproot" => &mut spam_by_type_by_era.taproot,
                            _ => &mut spam_by_type_by_era.inscriptions,
                        };
                        *era_map.entry(key).or_insert(0) += 1u64;
                    }
                }
                if sr.detected_types.contains(&SpamType::LargeWitness) {
                    largwitness_spam += 1;
                    if result.would_block && has_witness_violation(&result.violations) {
                        largwitness_and_witness_blocked += 1;
                    }
                }
                let confidence = spam_confidence(&sr.detected_types);
                *spam_by_confidence
                    .entry(format!("{:?}", confidence))
                    .or_insert(0) += 1;
                if result.would_block {
                    stats.spam_and_rule_blocked += 1;
                } else {
                    stats.spam_and_not_rule_blocked += 1;
                }
            } else if result.would_block {
                stats.rule_blocked_and_not_spam += 1;
                collateral_weight += w;
                *collateral_by_classification
                    .entry(class_key.clone())
                    .or_insert(0) += 1;
                for v in &result.violations {
                    let key = format!("{:?}", v);
                    *collateral_violations_by_type.entry(key).or_insert(0) += 1;
                    if let OutputSizeViolation::WitnessElementOversized(len) = v {
                        let bucket = collateral_witness_bucket(*len).to_string();
                        *collateral_witness_element_histogram
                            .entry(bucket)
                            .or_insert(0) += 1;
                    }
                }
            }
        }

        if !is_coinbase(tx) {
            for input in &tx.inputs {
                outpoint_index.remove(&input.prevout);
            }
        }
    }

    stats.violations_by_type = violations_by_type;
    stats.classifications = classifications;
    stats.spam_by_type = spam_by_type;
    stats.spam_by_confidence = spam_by_confidence;
    stats.witness_element_histogram = witness_element_histogram;
    stats.collateral_violations_by_type = collateral_violations_by_type;
    stats.collateral_witness_element_histogram = collateral_witness_element_histogram;
    stats.collateral_weight = collateral_weight;
    stats.largwitness_spam = largwitness_spam;
    stats.largwitness_and_witness_blocked = largwitness_and_witness_blocked;
    stats.blocked_txs_with_taproot_output = blocked_txs_with_taproot_output;
    stats.collateral_by_classification = collateral_by_classification;
    stats.spam_by_type_by_era = spam_by_type_by_era;

    stats
}

/// Update global results with block stats
pub fn merge_block_into_results(results: &mut ChainScanResults, block: &BlockScanStats) {
    results.blocks_scanned += 1;
    results.total_txs += block.total_txs as u64;
    results.blocked_txs += block.blocked_txs as u64;
    results.blocked_weight += block.blocked_weight;
    results.total_weight += block.total_weight;
    results.spam_txs += block.spam_txs as u64;

    results.blocked_txs_with_output_violation += block.blocked_txs_with_output_violation as u64;
    results.blocked_txs_with_witness_violation += block.blocked_txs_with_witness_violation as u64;
    results.blocked_txs_with_control_violation += block.blocked_txs_with_control_violation as u64;
    results.blocked_weight_with_output_violation += block.blocked_weight_with_output_violation;
    results.blocked_weight_with_witness_violation += block.blocked_weight_with_witness_violation;
    results.blocked_weight_with_control_violation += block.blocked_weight_with_control_violation;

    for (k, v) in &block.witness_element_histogram {
        *results
            .witness_element_histogram
            .entry(k.clone())
            .or_insert(0) += *v as u64;
    }
    for (k, v) in &block.collateral_violations_by_type {
        *results
            .collateral_violations_by_type
            .entry(k.clone())
            .or_insert(0) += *v as u64;
    }
    for (k, v) in &block.collateral_witness_element_histogram {
        *results
            .collateral_witness_element_histogram
            .entry(k.clone())
            .or_insert(0) += *v as u64;
    }

    // Retarget period (2016 blocks)
    let retarget_start = (block.height / 2016) * 2016;
    let entry = results
        .blocked_weight_by_retarget
        .entry(retarget_start)
        .or_insert_with(|| RetargetPeriodStats {
            start_height: retarget_start,
            blocked_weight: 0,
            total_weight: 0,
            blocked_weight_pct: 0.0,
        });
    entry.blocked_weight += block.blocked_weight;
    entry.total_weight += block.total_weight;
    entry.blocked_weight_pct = if entry.total_weight > 0 {
        entry.blocked_weight as f64 * 100.0 / entry.total_weight as f64
    } else {
        0.0
    };

    if block.height >= INSCRIPTIONS_START_HEIGHT {
        results.blocks_post_inscriptions += 1;
        results.total_txs_post_inscriptions += block.total_txs as u64;
        results.blocked_txs_post_inscriptions += block.blocked_txs as u64;
        results.blocked_weight_post_inscriptions += block.blocked_weight;
        results.total_weight_post_inscriptions += block.total_weight;
    }

    // Era breakdown
    let era = if block.height >= INSCRIPTIONS_START_HEIGHT {
        &mut results.era_inscriptions
    } else if block.height >= TAPROOT_START_HEIGHT {
        &mut results.era_taproot
    } else if block.height >= SEGWIT_START_HEIGHT {
        &mut results.era_segwit
    } else {
        &mut results.era_pre_segwit
    };
    era.blocks += 1;
    era.total_txs += block.total_txs as u64;
    era.blocked_txs += block.blocked_txs as u64;
    era.total_weight += block.total_weight;
    era.blocked_weight += block.blocked_weight;

    // Collateral by era
    let collateral_era = if block.height >= INSCRIPTIONS_START_HEIGHT {
        &mut results.collateral_by_era.inscriptions
    } else if block.height >= TAPROOT_START_HEIGHT {
        &mut results.collateral_by_era.taproot
    } else if block.height >= SEGWIT_START_HEIGHT {
        &mut results.collateral_by_era.segwit
    } else {
        &mut results.collateral_by_era.pre_segwit
    };
    collateral_era.collateral_txs += block.rule_blocked_and_not_spam as u64;
    collateral_era.collateral_weight += block.collateral_weight;

    // LargeWitness spam by era
    let lw_era = if block.height >= INSCRIPTIONS_START_HEIGHT {
        &mut results.largwitness_spam_by_era.inscriptions
    } else if block.height >= TAPROOT_START_HEIGHT {
        &mut results.largwitness_spam_by_era.taproot
    } else if block.height >= SEGWIT_START_HEIGHT {
        &mut results.largwitness_spam_by_era.segwit
    } else {
        &mut results.largwitness_spam_by_era.pre_segwit
    };
    *lw_era += block.largwitness_spam as u64;

    // Spam by type by era
    for (k, v) in &block.spam_by_type_by_era.pre_segwit {
        *results
            .spam_by_type_by_era
            .pre_segwit
            .entry(k.clone())
            .or_insert(0) += *v;
    }
    for (k, v) in &block.spam_by_type_by_era.segwit {
        *results
            .spam_by_type_by_era
            .segwit
            .entry(k.clone())
            .or_insert(0) += *v;
    }
    for (k, v) in &block.spam_by_type_by_era.taproot {
        *results
            .spam_by_type_by_era
            .taproot
            .entry(k.clone())
            .or_insert(0) += *v;
    }
    for (k, v) in &block.spam_by_type_by_era.inscriptions {
        *results
            .spam_by_type_by_era
            .inscriptions
            .entry(k.clone())
            .or_insert(0) += *v;
    }

    results.largwitness_and_witness_blocked += block.largwitness_and_witness_blocked as u64;
    results.blocked_txs_with_taproot_output += block.blocked_txs_with_taproot_output as u64;
    results.block_txs_with_tapscript_op_if_violation +=
        block.block_txs_with_tapscript_op_if_violation as u64;
    results.tapscript_op_if_grandfathered += block.tapscript_op_if_grandfathered as u64;
    results.tapscript_op_if_unspendable += block.tapscript_op_if_unspendable as u64;
    for (k, v) in &block.collateral_by_classification {
        *results
            .collateral_by_classification
            .entry(k.clone())
            .or_insert(0) += *v as u64;
    }

    results.spam_and_rule_blocked += block.spam_and_rule_blocked as u64;
    results.spam_and_not_rule_blocked += block.spam_and_not_rule_blocked as u64;
    results.rule_blocked_and_not_spam += block.rule_blocked_and_not_spam as u64;

    for (k, v) in &block.violations_by_type {
        *results.violations_by_type.entry(k.clone()).or_insert(0) += *v as u64;
    }
    for (k, v) in &block.classifications {
        *results.classifications.entry(k.clone()).or_insert(0) += *v as u64;
    }
    for (k, v) in &block.spam_by_type {
        *results.spam_by_type.entry(k.clone()).or_insert(0) += *v as u64;
    }
    for (k, v) in &block.spam_by_confidence {
        *results.spam_by_confidence.entry(k.clone()).or_insert(0) += *v as u64;
    }
}

/// Merge multiple ChainScanResults into one (for joining partial runs)
pub fn merge_results_into(acc: &mut ChainScanResults, other: &ChainScanResults) {
    acc.blocks_scanned += other.blocks_scanned;
    acc.total_txs += other.total_txs;
    acc.blocked_txs += other.blocked_txs;
    acc.blocked_weight += other.blocked_weight;
    acc.total_weight += other.total_weight;
    acc.blocks_post_inscriptions += other.blocks_post_inscriptions;
    acc.total_txs_post_inscriptions += other.total_txs_post_inscriptions;
    acc.blocked_txs_post_inscriptions += other.blocked_txs_post_inscriptions;
    acc.blocked_weight_post_inscriptions += other.blocked_weight_post_inscriptions;
    acc.total_weight_post_inscriptions += other.total_weight_post_inscriptions;

    acc.era_pre_segwit.blocks += other.era_pre_segwit.blocks;
    acc.era_pre_segwit.total_txs += other.era_pre_segwit.total_txs;
    acc.era_pre_segwit.blocked_txs += other.era_pre_segwit.blocked_txs;
    acc.era_pre_segwit.total_weight += other.era_pre_segwit.total_weight;
    acc.era_pre_segwit.blocked_weight += other.era_pre_segwit.blocked_weight;
    acc.era_segwit.blocks += other.era_segwit.blocks;
    acc.era_segwit.total_txs += other.era_segwit.total_txs;
    acc.era_segwit.blocked_txs += other.era_segwit.blocked_txs;
    acc.era_segwit.total_weight += other.era_segwit.total_weight;
    acc.era_segwit.blocked_weight += other.era_segwit.blocked_weight;
    acc.era_taproot.blocks += other.era_taproot.blocks;
    acc.era_taproot.total_txs += other.era_taproot.total_txs;
    acc.era_taproot.blocked_txs += other.era_taproot.blocked_txs;
    acc.era_taproot.total_weight += other.era_taproot.total_weight;
    acc.era_taproot.blocked_weight += other.era_taproot.blocked_weight;
    acc.era_inscriptions.blocks += other.era_inscriptions.blocks;
    acc.era_inscriptions.total_txs += other.era_inscriptions.total_txs;
    acc.era_inscriptions.blocked_txs += other.era_inscriptions.blocked_txs;
    acc.era_inscriptions.total_weight += other.era_inscriptions.total_weight;
    acc.era_inscriptions.blocked_weight += other.era_inscriptions.blocked_weight;

    acc.blocked_txs_with_output_violation += other.blocked_txs_with_output_violation;
    acc.blocked_txs_with_witness_violation += other.blocked_txs_with_witness_violation;
    acc.blocked_txs_with_control_violation += other.blocked_txs_with_control_violation;
    acc.blocked_weight_with_output_violation += other.blocked_weight_with_output_violation;
    acc.blocked_weight_with_witness_violation += other.blocked_weight_with_witness_violation;
    acc.blocked_weight_with_control_violation += other.blocked_weight_with_control_violation;

    acc.spam_txs += other.spam_txs;
    acc.spam_and_rule_blocked += other.spam_and_rule_blocked;
    acc.spam_and_not_rule_blocked += other.spam_and_not_rule_blocked;
    acc.rule_blocked_and_not_spam += other.rule_blocked_and_not_spam;

    for (k, v) in &other.violations_by_type {
        *acc.violations_by_type.entry(k.clone()).or_insert(0) += *v;
    }
    for (k, v) in &other.classifications {
        *acc.classifications.entry(k.clone()).or_insert(0) += *v;
    }
    for (k, v) in &other.spam_by_type {
        *acc.spam_by_type.entry(k.clone()).or_insert(0) += *v;
    }
    for (k, v) in &other.spam_by_confidence {
        *acc.spam_by_confidence.entry(k.clone()).or_insert(0) += *v;
    }
    for (k, v) in &other.witness_element_histogram {
        *acc.witness_element_histogram.entry(k.clone()).or_insert(0) += *v;
    }
    for (k, v) in &other.collateral_violations_by_type {
        *acc.collateral_violations_by_type
            .entry(k.clone())
            .or_insert(0) += *v;
    }
    for (k, v) in &other.collateral_witness_element_histogram {
        *acc.collateral_witness_element_histogram
            .entry(k.clone())
            .or_insert(0) += *v;
    }

    // Collateral by era
    acc.collateral_by_era.pre_segwit.collateral_txs +=
        other.collateral_by_era.pre_segwit.collateral_txs;
    acc.collateral_by_era.pre_segwit.collateral_weight +=
        other.collateral_by_era.pre_segwit.collateral_weight;
    acc.collateral_by_era.segwit.collateral_txs += other.collateral_by_era.segwit.collateral_txs;
    acc.collateral_by_era.segwit.collateral_weight +=
        other.collateral_by_era.segwit.collateral_weight;
    acc.collateral_by_era.taproot.collateral_txs += other.collateral_by_era.taproot.collateral_txs;
    acc.collateral_by_era.taproot.collateral_weight +=
        other.collateral_by_era.taproot.collateral_weight;
    acc.collateral_by_era.inscriptions.collateral_txs +=
        other.collateral_by_era.inscriptions.collateral_txs;
    acc.collateral_by_era.inscriptions.collateral_weight +=
        other.collateral_by_era.inscriptions.collateral_weight;

    // Blocked weight by retarget
    for (start, other_stats) in &other.blocked_weight_by_retarget {
        let entry = acc
            .blocked_weight_by_retarget
            .entry(*start)
            .or_insert_with(|| RetargetPeriodStats {
                start_height: *start,
                blocked_weight: 0,
                total_weight: 0,
                blocked_weight_pct: 0.0,
            });
        entry.blocked_weight += other_stats.blocked_weight;
        entry.total_weight += other_stats.total_weight;
        entry.blocked_weight_pct = if entry.total_weight > 0 {
            entry.blocked_weight as f64 * 100.0 / entry.total_weight as f64
        } else {
            0.0
        };
    }

    // LargeWitness spam by era
    acc.largwitness_spam_by_era.pre_segwit += other.largwitness_spam_by_era.pre_segwit;
    acc.largwitness_spam_by_era.segwit += other.largwitness_spam_by_era.segwit;
    acc.largwitness_spam_by_era.taproot += other.largwitness_spam_by_era.taproot;
    acc.largwitness_spam_by_era.inscriptions += other.largwitness_spam_by_era.inscriptions;

    for (k, v) in &other.spam_by_type_by_era.pre_segwit {
        *acc.spam_by_type_by_era
            .pre_segwit
            .entry(k.clone())
            .or_insert(0) += *v;
    }
    for (k, v) in &other.spam_by_type_by_era.segwit {
        *acc.spam_by_type_by_era.segwit.entry(k.clone()).or_insert(0) += *v;
    }
    for (k, v) in &other.spam_by_type_by_era.taproot {
        *acc.spam_by_type_by_era
            .taproot
            .entry(k.clone())
            .or_insert(0) += *v;
    }
    for (k, v) in &other.spam_by_type_by_era.inscriptions {
        *acc.spam_by_type_by_era
            .inscriptions
            .entry(k.clone())
            .or_insert(0) += *v;
    }

    acc.largwitness_and_witness_blocked += other.largwitness_and_witness_blocked;
    acc.blocked_txs_with_taproot_output += other.blocked_txs_with_taproot_output;
    for (k, v) in &other.collateral_by_classification {
        *acc.collateral_by_classification
            .entry(k.clone())
            .or_insert(0) += *v;
    }
}
