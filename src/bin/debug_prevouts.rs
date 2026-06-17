//! Debug tool to compare prevouts between individual verification and batch verification
//! This helps identify why scripts pass individually but fail in batch

use anyhow::{Context, Result};
use blvm_bench::chunked_cache::ChunkedBlockIterator;
use blvm_bench::sort_merge::verify::PrevoutReader;
use blvm_protocol::block::calculate_tx_id;
use blvm_protocol::types::TransactionOutput;
use std::collections::HashMap;
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: {} <block_height> <tx_idx> <input_idx>", args[0]);
        std::process::exit(1);
    }

    let block_height: u64 = args[1].parse().context("Invalid block height")?;
    let tx_idx: usize = args[2].parse().context("Invalid tx index")?;
    let input_idx: usize = args[3].parse().context("Invalid input index")?;

    let chunks_dir = blvm_bench::require_block_cache_dir()?;

    let prevouts_file =
        blvm_bench::block_cache_env::sort_merge_data_dir()?.join("joined_sorted.bin");

    println!("🔍 Debugging prevouts for:");
    println!("  Block: {}", block_height);
    println!("  Transaction index: {}", tx_idx);
    println!("  Input index: {}", input_idx);
    println!("");

    // Load block
    println!("📦 Loading block...");
    let mut block_iter = ChunkedBlockIterator::new(&chunks_dir, Some(block_height), Some(1))?
        .ok_or_else(|| anyhow::anyhow!("Failed to create block iterator"))?;

    let block_data = block_iter
        .next_block()?
        .ok_or_else(|| anyhow::anyhow!("Block {} not found", block_height))?;

    let (block, _witnesses) =
        blvm_protocol::serialization::block::deserialize_block_with_witnesses(&block_data)
            .context("Failed to deserialize block")?;

    let tx = block
        .transactions
        .get(tx_idx)
        .ok_or_else(|| anyhow::anyhow!("Transaction {} not found in block", tx_idx))?;

    let input = tx
        .inputs
        .get(input_idx)
        .ok_or_else(|| anyhow::anyhow!("Input {} not found in transaction", input_idx))?;

    println!(
        "  Transaction has {} inputs, {} outputs",
        tx.inputs.len(),
        tx.outputs.len()
    );
    println!("");

    // Load prevouts from joined_sorted.bin (what batch test uses)
    println!("📂 Loading prevouts from joined_sorted.bin (batch test method)...");
    let mut prevout_reader = PrevoutReader::new(&prevouts_file)?;
    let block_prevouts = prevout_reader.read_block_prevouts(block_height as u32)?;

    // Index by (tx_idx, input_idx)
    let mut prevout_map: HashMap<(u32, u32), &blvm_bench::sort_merge::merge_join::JoinedPrevout> =
        HashMap::new();
    for prevout in &block_prevouts {
        prevout_map.insert(
            (prevout.spending_tx_idx, prevout.spending_input_idx),
            prevout,
        );
    }

    println!(
        "  Found {} prevouts in joined_sorted.bin for this block",
        block_prevouts.len()
    );
    println!("");

    // Build intra-block UTXOs (what batch test uses)
    println!("🔗 Building intra-block UTXOs...");
    let mut intra_block_utxos: HashMap<_, TransactionOutput> = HashMap::new();
    for (tx_idx_iter, tx_iter) in block.transactions.iter().enumerate() {
        let tx_id = calculate_tx_id(tx_iter);
        for (output_idx, output) in tx_iter.outputs.iter().enumerate() {
            let outpoint = blvm_protocol::types::OutPoint {
                hash: tx_id,
                index: output_idx as u32,
            };
            intra_block_utxos.insert(
                outpoint,
                TransactionOutput {
                    value: output.value,
                    script_pubkey: output.script_pubkey.clone(),
                },
            );
        }
    }
    println!("  Built {} intra-block UTXOs", intra_block_utxos.len());
    println!("");

    // Build all_prevouts array (how batch test does it)
    println!("📋 Building all_prevouts array (batch test method)...");
    let mut all_prevouts_batch: Vec<TransactionOutput> = Vec::new();
    for (i, input_iter) in tx.inputs.iter().enumerate() {
        if let Some(prevout) = prevout_map.get(&(tx_idx as u32, i as u32)) {
            all_prevouts_batch.push(TransactionOutput {
                value: prevout.value,
                script_pubkey: prevout.script_pubkey.clone(),
            });
            println!(
                "  Input {}: Found in prevout_map (value: {}, script_pubkey: {} bytes)",
                i,
                prevout.value,
                prevout.script_pubkey.len()
            );
        } else if let Some(output) = intra_block_utxos.get(&input_iter.prevout) {
            all_prevouts_batch.push(output.clone());
            println!(
                "  Input {}: Found in intra_block_utxos (value: {}, script_pubkey: {} bytes)",
                i,
                output.value,
                output.script_pubkey.len()
            );
        } else {
            all_prevouts_batch.push(TransactionOutput {
                value: 0,
                script_pubkey: vec![],
            });
            println!("  Input {}: MISSING! (value: 0, script_pubkey: empty)", i);
        }
    }
    println!("  Built {} prevouts", all_prevouts_batch.len());
    println!("");

    // Now show what individual test uses (from block_prevouts directly)
    println!("📋 Building all_prevouts array (individual test method)...");
    let mut all_prevouts_individual: Vec<TransactionOutput> = Vec::new();
    for i in 0..tx.inputs.len() {
        let prevout_opt = block_prevouts
            .iter()
            .find(|p| p.spending_tx_idx == tx_idx as u32 && p.spending_input_idx == i as u32);
        if let Some(prevout) = prevout_opt {
            all_prevouts_individual.push(TransactionOutput {
                value: prevout.value,
                script_pubkey: prevout.script_pubkey.clone(),
            });
            println!(
                "  Input {}: Found (value: {}, script_pubkey: {} bytes)",
                i,
                prevout.value,
                prevout.script_pubkey.len()
            );
        } else {
            all_prevouts_individual.push(TransactionOutput {
                value: 0,
                script_pubkey: vec![],
            });
            println!("  Input {}: MISSING! (value: 0, script_pubkey: empty)", i);
        }
    }
    println!("  Built {} prevouts", all_prevouts_individual.len());
    println!("");

    // Compare the two arrays
    println!("🔍 Comparing prevouts arrays...");
    let mut differences = 0;
    for i in 0..tx.inputs.len() {
        let batch = &all_prevouts_batch[i];
        let individual = &all_prevouts_individual[i];

        if batch.value != individual.value || batch.script_pubkey != individual.script_pubkey {
            differences += 1;
            println!("  ❌ Input {} DIFFERS:", i);
            println!(
                "    Batch: value={}, script_pubkey={} bytes",
                batch.value,
                batch.script_pubkey.len()
            );
            println!(
                "    Individual: value={}, script_pubkey={} bytes",
                individual.value,
                individual.script_pubkey.len()
            );

            if batch.value != individual.value {
                println!(
                    "      Value mismatch: {} != {}",
                    batch.value, individual.value
                );
            }
            if batch.script_pubkey != individual.script_pubkey {
                println!("      ScriptPubkey mismatch (first 20 bytes):");
                println!(
                    "        Batch:      {}",
                    hex::encode(&batch.script_pubkey[..batch.script_pubkey.len().min(20)])
                );
                println!(
                    "        Individual: {}",
                    hex::encode(
                        &individual.script_pubkey[..individual.script_pubkey.len().min(20)]
                    )
                );
            }
        }
    }

    if differences == 0 {
        println!("  ✅ Arrays match perfectly!");
    } else {
        println!(
            "  ❌ Found {} differences - this explains why batch test fails!",
            differences
        );
    }

    Ok(())
}
