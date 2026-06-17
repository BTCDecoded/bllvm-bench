//! Analyze a specific missing prevout to understand why it's missing
//! Checks if the prevout txid exists in outputs_sorted.bin and determines source

use anyhow::{Context, Result};
use blvm_bench::chunked_cache::ChunkedBlockIterator;
use blvm_bench::sort_merge::output_refs::OutputRef;
use blvm_protocol::block::calculate_tx_id;
use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;

fn binary_search_txid(
    reader: &mut BufReader<File>,
    file_size: u64,
    target_txid: [u8; 32],
) -> Result<Option<OutputRef>> {
    let mut low = 0u64;
    let mut high = file_size;

    // Binary search for the txid
    while low < high {
        let mid = (low + high) / 2;

        // Try to find a record boundary near mid
        let mut search_offset = mid;
        let mut found_record = false;

        // Search backwards for a valid record start
        for _i in 0..1000 {
            if search_offset < 100 {
                search_offset = 0;
            } else {
                search_offset -= 100;
            }

            reader.seek(SeekFrom::Start(search_offset))?;
            let mut header = vec![0u8; 200];
            let n = reader.read(&mut header)?;
            if n < 51 {
                continue;
            }

            if let Some((record, _)) = OutputRef::from_bytes(&header) {
                found_record = true;
                match record.txid.cmp(&target_txid) {
                    std::cmp::Ordering::Equal => {
                        return Ok(Some(record));
                    }
                    std::cmp::Ordering::Less => {
                        low = search_offset + 200; // Rough estimate
                        break;
                    }
                    std::cmp::Ordering::Greater => {
                        high = search_offset;
                        break;
                    }
                }
            }
        }

        if !found_record {
            low = mid + 1;
        }
    }

    Ok(None)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: {} <block_height> <tx_idx> <input_idx>", args[0]);
        eprintln!("Example: {} 900848 3712 8", args[0]);
        std::process::exit(1);
    }

    let block_height: u64 = args[1].parse()?;
    let tx_idx: usize = args[2].parse()?;
    let input_idx: usize = args[3].parse()?;

    let chunks_dir = blvm_bench::require_block_cache_dir()?;

    let outputs_file =
        blvm_bench::block_cache_env::sort_merge_data_dir()?.join("outputs_sorted.bin");

    println!("🔍 Analyzing missing prevout:");
    println!("  Block: {}", block_height);
    println!("  Transaction index: {}", tx_idx);
    println!("  Input index: {}", input_idx);
    println!("");

    // Load the block
    let mut block_iter = ChunkedBlockIterator::new(&chunks_dir, Some(block_height), Some(1))?
        .ok_or_else(|| anyhow::anyhow!("Failed to create block iterator"))?;

    let block_data = block_iter
        .next_block()?
        .ok_or_else(|| anyhow::anyhow!("Block {} not found", block_height))?;

    let (block, _witnesses) =
        deserialize_block_with_witnesses(&block_data).context("Failed to deserialize block")?;

    if tx_idx >= block.transactions.len() {
        return Err(anyhow::anyhow!(
            "Transaction index {} out of range (block has {} transactions)",
            tx_idx,
            block.transactions.len()
        ));
    }

    let tx = &block.transactions[tx_idx];
    if input_idx >= tx.inputs.len() {
        return Err(anyhow::anyhow!(
            "Input index {} out of range (transaction has {} inputs)",
            input_idx,
            tx.inputs.len()
        ));
    }

    let input = &tx.inputs[input_idx];
    let prevout_txid = input.prevout.hash;
    let prevout_idx = input.prevout.index;

    println!("📋 Input details:");
    println!("  Prevout txid: {}", hex::encode(prevout_txid));
    println!("  Prevout index: {}", prevout_idx);
    println!("");

    // Check if this is a null prevout (coinbase)
    if prevout_txid == [0u8; 32] && prevout_idx == 0xffffffff {
        println!("  ⚠️  WARNING: This is a NULL prevout (coinbase input)!");
        println!("     Coinbase inputs should be skipped in input extraction.");
        println!(
            "     This suggests a bug in extract_input_refs - it's including coinbase inputs!"
        );
        return Ok(());
    }

    // Check if prevout_txid starts with 0xff (suspicious)
    if prevout_txid[0] == 0xff {
        println!("  ⚠️  WARNING: Prevout txid starts with 0xff (suspicious)!");
        println!("     Valid Bitcoin txids are SHA256 hashes (random-looking).");
        println!("     This might indicate:");
        println!("     1. Invalid/orphaned transaction");
        println!("     2. Data corruption");
        println!("     3. Extraction bug");
    }

    // Calculate the actual transaction ID to verify
    let actual_tx_id = calculate_tx_id(tx);
    println!("  Actual transaction ID: {}", hex::encode(actual_tx_id));
    println!("");

    // Check if this txid exists in outputs_sorted.bin
    println!("🔎 Checking outputs_sorted.bin...");
    let file = File::open(&outputs_file)?;
    let file_size = file.metadata()?.len();
    let mut reader = BufReader::with_capacity(64 * 1024 * 1024, file);

    if let Some(output) = binary_search_txid(&mut reader, file_size, prevout_txid)? {
        println!("  ✅ FOUND in outputs_sorted.bin!");
        println!("  Block height: {}", output.block_height);
        println!("  Output index: {}", output.output_idx);
        println!("  Is coinbase: {}", output.is_coinbase);
        println!("  Value: {} satoshis", output.value);
        println!("");

        if output.output_idx != prevout_idx {
            println!("  ⚠️  WARNING: Output index mismatch!");
            println!(
                "     Expected: {}, Found: {}",
                prevout_idx, output.output_idx
            );
        } else {
            println!("  ✅ Output index matches!");
        }

        if output.block_height > 912723 {
            println!(
                "  ⚠️  WARNING: Output is from block {} > 912723",
                output.block_height
            );
            println!("     This shouldn't be in outputs_sorted.bin if we only processed 0-912723!");
        } else {
            println!("  ✅ Output is from processed block range (0-912723)");
        }
    } else {
        println!("  ❌ NOT FOUND in outputs_sorted.bin");
        println!("");
        println!("  Possible reasons:");
        println!("  1. Transaction was created in block > 912723 (expected)");
        println!("  2. Transaction was created in block 0-912723 but not extracted (bug)");
        println!("  3. Txid calculation mismatch (bug)");
        println!("");
        println!("  To verify, we'd need to search the blockchain for this txid.");
    }

    Ok(())
}
