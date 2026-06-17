//! Quick check a specific failure without all the overhead
//! Usage: quick_check_failure <block> <tx_idx> <input_idx>

use anyhow::{Context, Result};
use blvm_protocol::Witness;
use blvm_protocol::block::calculate_script_flags_for_block_network;
use blvm_protocol::script::verify_script_with_context_full;
use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
use blvm_protocol::transaction::is_coinbase;
use blvm_protocol::types::{Network, TransactionOutput};
use blvm_protocol::witness::is_witness_empty;

use blvm_bench::chunked_cache::ChunkedBlockIterator;
use blvm_bench::sort_merge::verify::PrevoutReader;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: quick_check_failure <block_height> <tx_idx> <input_idx>");
        std::process::exit(1);
    }

    let block_height: u64 = args[1].parse().context("Invalid block height")?;
    let tx_idx: usize = args[2].parse().context("Invalid tx index")?;
    let input_idx: usize = args[3].parse().context("Invalid input index")?;

    println!(
        "Quick check: block {}, tx {}, input {}",
        block_height, tx_idx, input_idx
    );

    // Load block
    let chunks_dir = blvm_bench::require_block_cache_dir()?;
    let mut block_iter = ChunkedBlockIterator::new(&chunks_dir, Some(block_height), Some(1))?
        .ok_or_else(|| anyhow::anyhow!("Failed to create block iterator"))?;

    let block_data = block_iter
        .next_block()?
        .ok_or_else(|| anyhow::anyhow!("Block {} not found", block_height))?;

    let (block, witnesses) =
        deserialize_block_with_witnesses(&block_data).context("Failed to deserialize block")?;

    if tx_idx >= block.transactions.len() {
        anyhow::bail!("Transaction index {} out of range", tx_idx);
    }

    let tx = &block.transactions[tx_idx];
    if is_coinbase(tx) {
        println!("Coinbase transaction - skipping");
        return Ok(());
    }

    if input_idx >= tx.inputs.len() {
        anyhow::bail!("Input index {} out of range", input_idx);
    }

    let input = &tx.inputs[input_idx];

    // Load prevouts
    let prevouts_file = chunks_dir.join("sort_merge_data/joined_sorted.bin");
    let mut prevout_reader = PrevoutReader::new(&prevouts_file)?;
    prevout_reader.skip_to_block(block_height as u32)?;
    let block_prevouts = prevout_reader.read_block_prevouts(block_height as u32)?;

    // Find prevout
    let prevout = block_prevouts
        .iter()
        .find(|p| p.spending_tx_idx == tx_idx as u32 && p.spending_input_idx == input_idx as u32)
        .ok_or_else(|| anyhow::anyhow!("Prevout not found"))?;

    // Build all prevouts
    let mut all_prevouts = Vec::new();
    for i in 0..tx.inputs.len() {
        if let Some(p) = block_prevouts
            .iter()
            .find(|p| p.spending_tx_idx == tx_idx as u32 && p.spending_input_idx == i as u32)
        {
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

    // Get witness
    let tx_witness = witnesses.get(tx_idx);
    let witness_stack: Option<&Witness> = tx_witness.and_then(|w| w.get(input_idx));

    let wits = witnesses.get(tx_idx).map(|w| w.as_slice()).unwrap_or(&[]);
    let has_witness = wits.iter().any(|wit| !is_witness_empty(wit));
    let tx_flags =
        calculate_script_flags_for_block_network(tx, has_witness, block_height, Network::Mainnet);

    println!("Flags: 0x{:x}", tx_flags);
    println!("Script sig: {} bytes", input.script_sig.len());
    println!("Script pubkey: {} bytes", prevout.script_pubkey.len());
    println!("Witness: {:?}", witness_stack.map(|w| w.len()));

    let prevout_values: Vec<i64> = all_prevouts.iter().map(|o| o.value).collect();
    let prevout_script_pubkeys: Vec<&[u8]> = all_prevouts
        .iter()
        .map(|o| o.script_pubkey.as_slice())
        .collect();

    // Verify
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
        None,
        Network::Mainnet,
        blvm_protocol::script::SigVersion::Base,
        None,
        None,
        None,
        None,
        None,
    ) {
        Ok(true) => {
            println!("✅ PASSED - but test said it failed!");
        }
        Ok(false) => {
            println!("❌ FAILED (returned false)");
        }
        Err(e) => {
            println!("❌ ERROR: {:?}", e);
        }
    }

    Ok(())
}
