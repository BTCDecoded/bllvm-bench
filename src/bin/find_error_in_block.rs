//! Find script errors in a specific block (mirrors sort-merge step6: collector + batch).

use anyhow::{Context, Result};
use blvm_bench::chunked_cache::ChunkedBlockIterator;
use blvm_bench::sort_merge::merge_join::JoinedPrevout;
use blvm_protocol::activation::{ForkActivationTable, IsForkActive};
use blvm_protocol::bip113::get_median_time_past;
use blvm_protocol::block::{
    calculate_base_script_flags_for_block_network, tx_has_nonempty_input_witness,
    tx_requires_witness_script_flags,
};
use blvm_protocol::script::{SigVersion, verify_script_with_context_full};
use blvm_protocol::segwit::Witness;
use blvm_protocol::serialization::block::{
    deserialize_block_header, deserialize_block_with_witnesses,
};
use blvm_protocol::transaction::is_coinbase;
use blvm_protocol::types::ForkId;
use blvm_protocol::types::{Network, TransactionOutput};
use blvm_protocol::witness::is_witness_empty;
use std::collections::HashMap;

#[cfg(feature = "production")]
use blvm_consensus::bip348::SchnorrSignatureCollector;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: find_error_in_block <block_height>");
        std::process::exit(1);
    }

    let block_height: u64 = args[1].parse().context("Invalid block height")?;

    println!("🔍 Finding script errors in block {}...", block_height);

    let block_data = if let Ok(path) = std::env::var("BLOCK_FILE") {
        std::fs::read(&path).with_context(|| format!("Failed to read BLOCK_FILE: {path}"))?
    } else {
        let chunks_dir = blvm_bench::require_block_cache_dir()?;
        let mut block_iter = ChunkedBlockIterator::new(&chunks_dir, Some(block_height), Some(1))?
            .ok_or_else(|| anyhow::anyhow!("Failed to create block iterator"))?;
        block_iter
            .next_block()?
            .ok_or_else(|| anyhow::anyhow!("Block {} not found", block_height))?
    };

    let block_header = deserialize_block_header(&block_data[..80.min(block_data.len())])?;
    let median_time_past = Some(get_median_time_past(&[block_header.clone()]));

    let (block, witnesses) =
        deserialize_block_with_witnesses(&block_data).context("Failed to deserialize block")?;

    let prevouts_file =
        blvm_bench::block_cache_env::sort_merge_data_dir()?.join("joined_sorted.bin");

    use blvm_bench::sort_merge::verify::PrevoutReader;
    let mut prevout_reader = PrevoutReader::new(&prevouts_file)?;
    prevout_reader.skip_to_block(block_height as u32)?;
    let block_prevouts = prevout_reader.read_block_prevouts(block_height as u32)?;

    println!(
        "  Loaded {} prevouts for block {}",
        block_prevouts.len(),
        block_height
    );

    let mut prevout_map: HashMap<(u32, u32), &JoinedPrevout> = HashMap::new();
    for prevout in &block_prevouts {
        prevout_map.insert(
            (prevout.spending_tx_idx, prevout.spending_input_idx),
            prevout,
        );
    }

    let network = Network::Mainnet;
    let activation = ForkActivationTable::from_network(network);
    let base_flags = {
        let f = calculate_base_script_flags_for_block_network(block_height, network);
        if activation.is_fork_active(ForkId::Taproot, block_height) {
            f | 0x20000
        } else {
            f
        }
    };
    let height_has_segwit = activation.is_fork_active(ForkId::SegWit, block_height);
    let height_has_taproot = activation.is_fork_active(ForkId::Taproot, block_height);

    if std::env::var("STEP6_MODE").is_ok() {
        use blvm_bench::sort_merge::verify::verify_block_like_step6;
        let result =
            verify_block_like_step6(block_height, block_data, block_prevouts.clone(), network)?;
        println!(
            "  step6: missing={} per_input_false={} schnorr_batch={}/{} invalid",
            result.missing_prevouts,
            result.per_input_false.len(),
            result.schnorr_batch_invalid,
            result.schnorr_batch_total
        );
        for (tx_idx, input_idx) in &result.per_input_false {
            println!("  ↳ per-input FALSE: tx {}, input {}", tx_idx, input_idx);
        }
        for (tx_idx, input_idx) in &result.schnorr_failures {
            let spk = prevout_map
                .get(&(*tx_idx as u32, *input_idx as u32))
                .map(|p| hex::encode(&p.script_pubkey))
                .unwrap_or_default();
            println!(
                "  ↳ schnorr failure: tx {}, input {} spk={spk}",
                tx_idx, input_idx
            );
        }
        return Ok(());
    }

    let mut input_false = 0u32;
    let mut input_err = 0u32;

    #[cfg(feature = "production")]
    let schnorr_collector = SchnorrSignatureCollector::new();

    for (tx_idx, tx) in block.transactions.iter().enumerate() {
        if is_coinbase(tx) {
            continue;
        }

        let tx_witnesses = witnesses.get(tx_idx);
        let has_real_witness =
            tx_has_nonempty_input_witness(tx_witnesses.as_ref().map(|w| w.as_slice()));
        let mut tx_flags = base_flags;
        if height_has_segwit && tx_requires_witness_script_flags(tx, has_real_witness) {
            tx_flags |= 0x800;
        }
        if height_has_taproot {
            use blvm_protocol::constants::TAPROOT_SCRIPT_LENGTH;
            for o in &tx.outputs {
                let s = &o.script_pubkey;
                if s.len() == TAPROOT_SCRIPT_LENGTH
                    && s[0] == blvm_protocol::opcodes::OP_1
                    && s[1] == blvm_protocol::opcodes::PUSH_32_BYTES
                {
                    tx_flags |= 0x8000;
                    break;
                }
            }
        }

        for (input_idx, input) in tx.inputs.iter().enumerate() {
            let Some(prevout) = prevout_map.get(&(tx_idx as u32, input_idx as u32)) else {
                continue;
            };
            let prevout_script = prevout.script_pubkey.clone();
            let witness_stack: Option<&Witness> = tx_witnesses
                .and_then(|wits| wits.get(input_idx))
                .filter(|w| !is_witness_empty(w));

            let mut all_prevouts = Vec::new();
            for (i, _input) in tx.inputs.iter().enumerate() {
                if let Some(p) = prevout_map.get(&(tx_idx as u32, i as u32)) {
                    all_prevouts.push(TransactionOutput {
                        value: p.value,
                        script_pubkey: p.script_pubkey.clone(),
                    });
                } else {
                    all_prevouts.push(TransactionOutput {
                        value: 0,
                        script_pubkey: vec![],
                    });
                }
            }

            let prevout_values: Vec<i64> = all_prevouts.iter().map(|o| o.value).collect();
            let prevout_script_pubkeys: Vec<&[u8]> = all_prevouts
                .iter()
                .map(|o| o.script_pubkey.as_slice())
                .collect();

            #[cfg(feature = "production")]
            let collector_ref = Some(&schnorr_collector);
            #[cfg(not(feature = "production"))]
            let collector_ref = None;

            match verify_script_with_context_full(
                &input.script_sig,
                &prevout_script,
                witness_stack,
                tx_flags,
                tx,
                input_idx,
                &prevout_values,
                &prevout_script_pubkeys,
                Some(block_height),
                median_time_past,
                network,
                SigVersion::Base,
                collector_ref,
                None,
                None,
                None,
                None,
            ) {
                Ok(true) => {}
                Ok(false) => {
                    input_false += 1;
                    println!(
                        "❌ FALSE at block {}, tx {}, input {}",
                        block_height, tx_idx, input_idx
                    );
                    println!("   spk: {}", hex::encode(&prevout_script));
                }
                Err(e) => {
                    input_err += 1;
                    println!(
                        "❌ ERR at block {}, tx {}, input {}: {:?}",
                        block_height, tx_idx, input_idx, e
                    );
                }
            }
        }
    }

    #[cfg(feature = "production")]
    {
        match schnorr_collector.verify_batch() {
            Ok(batch) => {
                let bad = batch.iter().filter(|&&v| !v).count();
                if bad > 0 {
                    println!(
                        "❌ Schnorr batch: {bad}/{} invalid signatures in block {}",
                        batch.len(),
                        block_height
                    );
                    // Re-verify without batch deferral to locate failing inputs.
                    if std::env::var("LOCATE_SCHNORR_FAILURES").is_ok() {
                        locate_schnorr_failures(
                            block_height,
                            &block,
                            &witnesses,
                            &prevout_map,
                            median_time_past,
                            network,
                            base_flags,
                            height_has_segwit,
                            height_has_taproot,
                        )?;
                    }
                } else if !batch.is_empty() {
                    println!("  Schnorr batch: {} sigs OK", batch.len());
                }
            }
            Err(e) => println!("❌ Schnorr batch error: {:?}", e),
        }
    }

    if input_false == 0 && input_err == 0 {
        #[cfg(feature = "production")]
        {
            println!(
                "  ✅ No per-input script failures in block {}",
                block_height
            );
        }
        #[cfg(not(feature = "production"))]
        println!("  ✅ No errors found in block {}", block_height);
    }

    Ok(())
}

#[cfg(feature = "production")]
fn locate_schnorr_failures(
    block_height: u64,
    block: &blvm_protocol::types::Block,
    witnesses: &[Vec<Witness>],
    prevout_map: &HashMap<(u32, u32), &JoinedPrevout>,
    median_time_past: Option<u64>,
    network: Network,
    base_flags: u32,
    height_has_segwit: bool,
    height_has_taproot: bool,
) -> Result<()> {
    use blvm_protocol::constants::TAPROOT_SCRIPT_LENGTH;

    for (tx_idx, tx) in block.transactions.iter().enumerate() {
        if is_coinbase(tx) {
            continue;
        }
        let tx_witnesses = witnesses.get(tx_idx);
        let has_real_witness =
            tx_has_nonempty_input_witness(tx_witnesses.as_ref().map(|w| w.as_slice()));
        let mut tx_flags = base_flags;
        if height_has_segwit && tx_requires_witness_script_flags(tx, has_real_witness) {
            tx_flags |= 0x800;
        }
        if height_has_taproot {
            for o in &tx.outputs {
                let s = &o.script_pubkey;
                if s.len() == TAPROOT_SCRIPT_LENGTH
                    && s[0] == blvm_protocol::opcodes::OP_1
                    && s[1] == blvm_protocol::opcodes::PUSH_32_BYTES
                {
                    tx_flags |= 0x8000;
                    break;
                }
            }
        }

        for (input_idx, input) in tx.inputs.iter().enumerate() {
            let Some(prevout) = prevout_map.get(&(tx_idx as u32, input_idx as u32)) else {
                continue;
            };
            let witness_stack: Option<&Witness> = tx_witnesses
                .and_then(|wits| wits.get(input_idx))
                .filter(|w| !is_witness_empty(w));

            let mut all_prevouts = Vec::new();
            for (i, _) in tx.inputs.iter().enumerate() {
                if let Some(p) = prevout_map.get(&(tx_idx as u32, i as u32)) {
                    all_prevouts.push(TransactionOutput {
                        value: p.value,
                        script_pubkey: p.script_pubkey.clone(),
                    });
                } else {
                    all_prevouts.push(TransactionOutput {
                        value: 0,
                        script_pubkey: vec![],
                    });
                }
            }
            let prevout_values: Vec<i64> = all_prevouts.iter().map(|o| o.value).collect();
            let prevout_script_pubkeys: Vec<&[u8]> = all_prevouts
                .iter()
                .map(|o| o.script_pubkey.as_slice())
                .collect();

            match verify_script_with_context_full(
                &input.script_sig,
                &prevout.script_pubkey,
                witness_stack,
                tx_flags,
                tx,
                input_idx,
                &prevout_values,
                &prevout_script_pubkeys,
                Some(block_height),
                median_time_past,
                network,
                SigVersion::Base,
                None,
                None,
                None,
                None,
                None,
            ) {
                Ok(false) => {
                    println!(
                        "  ↳ immediate verify FALSE: tx {}, input {} spk={}",
                        tx_idx,
                        input_idx,
                        hex::encode(&prevout.script_pubkey)
                    );
                }
                Err(e) => {
                    println!(
                        "  ↳ immediate verify ERR: tx {}, input {}: {:?}",
                        tx_idx, input_idx, e
                    );
                }
                Ok(true) => {}
            }
        }
    }
    Ok(())
}
