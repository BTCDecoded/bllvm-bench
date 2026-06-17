//! Script-level differential validation using Bitcoin Core's `libbitcoinconsensus`.
//!
//! Compares script verification between BLVM and Core for a single input when full
//! prevout data is available.

use anyhow::{Result, bail};
use bitcoinconsensus::{VERIFY_ALL_PRE_TAPROOT, verify_with_flags};
use blvm_protocol::script::{SigVersion, verify_script_with_context_full};
use blvm_protocol::serialization::transaction::serialize_transaction;
use blvm_protocol::types::{Block, ByteString, Network, Transaction};

/// Script comparison result for one input.
#[derive(Debug, Clone)]
pub struct ScriptComparisonResult {
    pub matches: bool,
    pub core_result: bool,
    pub blvm_result: bool,
    pub script_pubkey: ByteString,
    pub input_index: usize,
    pub tx_hash: Option<String>,
}

/// Compare script verification for one input, using a full prevout row per vin (BLVM sighash).
pub fn compare_script_verification(
    prevout_values: &[i64],
    prevout_script_pubkeys: &[&[u8]],
    script_sig: &ByteString,
    tx: &Transaction,
    input_index: usize,
    consensus_flags: u32,
) -> Result<ScriptComparisonResult> {
    if prevout_values.len() != tx.inputs.len() || prevout_script_pubkeys.len() != tx.inputs.len() {
        bail!(
            "prevout slices (values={}, scripts={}) must match input count {}",
            prevout_values.len(),
            prevout_script_pubkeys.len(),
            tx.inputs.len()
        );
    }
    if input_index >= tx.inputs.len() {
        bail!("input_index out of range");
    }

    let script_pubkey = prevout_script_pubkeys[input_index];
    let amount = prevout_values[input_index] as u64;

    let tx_bytes = serialize_transaction(tx);

    let core_ok = verify_with_flags(
        script_pubkey,
        amount,
        &tx_bytes,
        None,
        input_index,
        consensus_flags,
    )
    .is_ok();

    // P2SH | STRICTENC | DERSIG | LOW_S | NULLDUMMY | WITNESS (legacy test flags)
    let blvm_flags: u32 = 0x01 | 0x02 | 0x04 | 0x08 | 0x10 | 0x800;

    let spk_owned: ByteString = script_pubkey.to_vec().into();
    let blvm_result = verify_script_with_context_full(
        script_sig,
        script_pubkey,
        None,
        blvm_flags,
        tx,
        input_index,
        prevout_values,
        prevout_script_pubkeys,
        None,
        None,
        Network::Mainnet,
        SigVersion::Base,
        None,
        None,
        None,
        None,
        None,
    )?;

    let matches = core_ok == blvm_result;

    let tx_hash = if !matches {
        use sha2::{Digest, Sha256};
        let tx_bytes = serialize_transaction(tx);
        let hash = Sha256::digest(Sha256::digest(&tx_bytes));
        Some(hex::encode(hash))
    } else {
        None
    };

    Ok(ScriptComparisonResult {
        matches,
        core_result: core_ok,
        blvm_result,
        script_pubkey: spk_owned,
        input_index,
        tx_hash,
    })
}

/// Compare all scripts in a block (placeholder until UTXO-aware path is wired).
pub fn compare_block_scripts(block: &Block, height: u64) -> Result<Vec<ScriptComparisonResult>> {
    let results = Vec::new();

    for (tx_idx, tx) in block.transactions.iter().enumerate() {
        if tx_idx == 0 {
            continue;
        }
        for (input_idx, _input) in tx.inputs.iter().enumerate() {
            eprintln!(
                "[script_validation] block {height} tx {tx_idx} input {input_idx}: \
                 skipping script compare (needs UTXO set)"
            );
        }
    }

    Ok(results)
}

/// Full block compare when `(script_pubkey, amount)` per input is already known.
///
/// **Layout:** `utxos` is applied to **each** non-coinbase transaction. Only use when that
/// matches your data (typically **coinbase + one spending transaction** in `block`).
pub fn compare_block_scripts_with_utxos(
    block: &Block,
    height: u64,
    utxos: &[(ByteString, u64)],
) -> Result<Vec<ScriptComparisonResult>> {
    let mut results = Vec::new();
    let flags = VERIFY_ALL_PRE_TAPROOT;

    if block.transactions.len() > 2 {
        eprintln!(
            "[script_validation] compare_block_scripts_with_utxos at height {height}: \
             expected at most coinbase + one tx (got {} txs); skipping",
            block.transactions.len()
        );
        return Ok(results);
    }

    for (tx_idx, tx) in block.transactions.iter().enumerate() {
        if tx_idx == 0 {
            continue;
        }

        if utxos.len() != tx.inputs.len() {
            eprintln!(
                "[script_validation] block {height} tx {tx_idx}: utxo row count {} != inputs {}",
                utxos.len(),
                tx.inputs.len()
            );
            continue;
        }

        let prevout_values: Vec<i64> = utxos.iter().map(|(_, a)| *a as i64).collect();
        let prevout_script_pubkeys: Vec<&[u8]> =
            utxos.iter().map(|(spk, _)| spk.as_slice()).collect();

        for (input_idx, input) in tx.inputs.iter().enumerate() {
            let script_sig = &input.script_sig;

            match compare_script_verification(
                &prevout_values,
                &prevout_script_pubkeys,
                script_sig,
                tx,
                input_idx,
                flags,
            ) {
                Ok(result) => {
                    if !result.matches {
                        eprintln!(
                            "[script_validation] divergence block {height} tx {tx_idx} \
                             input {input_idx}: core={} blvm={}",
                            result.core_result, result.blvm_result
                        );
                    }
                    results.push(result);
                }
                Err(e) => {
                    eprintln!(
                        "[script_validation] compare failed block {height} tx {tx_idx} \
                         input {input_idx}: {e}"
                    );
                }
            }
        }
    }

    Ok(results)
}
