//! Analyze a specific script failure to understand the root cause
//! Compares BLVM behavior with Bitcoin Core

use anyhow::{Context, Result};
use blvm_bench::chunked_cache::ChunkedBlockIterator;
use blvm_bench::remote_core_rpc::RemoteCoreRpcClient;
use blvm_protocol::block::calculate_tx_id;
use blvm_protocol::script::{SigVersion, verify_script_with_context_full};
use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
use blvm_protocol::serialization::transaction::serialize_transaction;
use blvm_protocol::types::Network;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: {} <block_height> <tx_idx> <input_idx>", args[0]);
        eprintln!("Example: {} 445000 1 0", args[0]);
        std::process::exit(1);
    }

    let block_height: u64 = args[1].parse()?;
    let tx_idx: usize = args[2].parse()?;
    let input_idx: usize = args[3].parse()?;

    let chunks_dir = blvm_bench::require_block_cache_dir()?;

    println!("🔍 Analyzing script failure:");
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

    let (block, witnesses) =
        deserialize_block_with_witnesses(&block_data).context("Failed to deserialize block")?;

    let tx = &block.transactions[tx_idx];
    let input = &tx.inputs[input_idx];
    let tx_id = calculate_tx_id(tx);

    println!("📋 Transaction details:");
    println!("  Transaction ID: {}", hex::encode(tx_id));
    println!("  Script sig: {}", hex::encode(&input.script_sig));
    println!("  Script sig length: {} bytes", input.script_sig.len());
    println!("");

    // Load prevout from joined_sorted.bin
    println!("🔎 Loading prevout...");
    use blvm_bench::sort_merge::merge_join::JoinedPrevout;
    use std::fs::File;
    use std::io::{BufReader, Read, Seek, SeekFrom};

    let joined_file = blvm_bench::block_cache_env::sort_merge_data_dir()?.join("joined_sorted.bin");
    let file = File::open(&joined_file)?;
    let file_size = file.metadata()?.len();
    let mut reader = BufReader::new(file);

    // Search for the joined prevout by spending location
    let mut found_prevout: Option<JoinedPrevout> = None;
    let mut buf = vec![0u8; 1024 * 1024]; // 1MB buffer

    reader.seek(SeekFrom::Start(0))?;
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            break;
        }

        let mut pos = 0;
        while pos < n {
            if let Some((joined, consumed)) = JoinedPrevout::from_bytes(&buf[pos..]) {
                if joined.spending_block == block_height as u32
                    && joined.spending_tx_idx == tx_idx as u32
                    && joined.spending_input_idx == input_idx as u32
                {
                    found_prevout = Some(joined);
                    break;
                }
                pos += consumed;
            } else {
                break;
            }
        }

        if found_prevout.is_some() {
            break;
        }
    }

    if let Some(joined) = found_prevout {
        println!("  ✅ Found prevout!");
        println!("    Script pubkey: {}", hex::encode(&joined.script_pubkey));
        println!(
            "    Script pubkey length: {} bytes",
            joined.script_pubkey.len()
        );
        println!("    Value: {} satoshis", joined.value);
        println!("");

        // Try to verify with BLVM
        println!("🔐 Verifying with BLVM...");
        let tx_witnesses = witnesses.get(tx_idx);
        let witness_stack = tx_witnesses.and_then(|w| w.get(input_idx));

        // Build prevouts list
        let mut prevouts = Vec::new();
        for (i, input) in tx.inputs.iter().enumerate() {
            if i == input_idx {
                prevouts.push(blvm_protocol::types::TransactionOutput {
                    value: joined.value,
                    script_pubkey: joined.script_pubkey.clone(),
                });
            } else {
                // Placeholder for other inputs
                prevouts.push(blvm_protocol::types::TransactionOutput {
                    value: 0,
                    script_pubkey: vec![],
                });
            }
        }

        // Calculate script flags (optimization: just check witness presence, no flattening)
        let has_witness = tx_witnesses.map(|w| !w.is_empty()).unwrap_or(false);
        let flags = blvm_protocol::block::calculate_script_flags_for_block_network(
            tx,
            has_witness,
            block_height,
            Network::Mainnet,
        );
        println!("  Script flags: 0x{:x}", flags);

        let prevout_values: Vec<i64> = prevouts.iter().map(|o| o.value).collect();
        let prevout_script_pubkeys: Vec<&[u8]> = prevouts
            .iter()
            .map(|o| o.script_pubkey.as_slice())
            .collect();

        match verify_script_with_context_full(
            &input.script_sig,
            &joined.script_pubkey,
            witness_stack,
            flags,
            tx,
            input_idx,
            &prevout_values,
            &prevout_script_pubkeys,
            Some(block_height),
            None,
            Network::Mainnet,
            SigVersion::Base,
            None,
            None,
            None,
            None,
            None,
        ) {
            Ok(true) => {
                println!("  ✅ BLVM verification: PASSED");
                println!("  ⚠️  But step6 reported failure - this is unexpected!");
            }
            Ok(false) => {
                println!("  ❌ BLVM verification: FAILED (returned false)");
                println!("  💡 This matches the step6 failure report");
                println!("  🔍 Investigating why script returned false...");

                // Try to get more details about why it failed
                // Check if it's a signature verification issue
                if input.script_sig.len() > 0 && joined.script_pubkey.len() > 0 {
                    println!(
                        "  Script sig starts with: {}",
                        hex::encode(&input.script_sig[..input.script_sig.len().min(20)])
                    );
                    println!(
                        "  Script pubkey starts with: {}",
                        hex::encode(&joined.script_pubkey[..joined.script_pubkey.len().min(20)])
                    );

                    // Check if it's a P2PKH script
                    if joined.script_pubkey.len() == 25
                        && joined.script_pubkey[0] == blvm_protocol::opcodes::OP_DUP
                        && joined.script_pubkey[1] == blvm_protocol::opcodes::OP_HASH160
                        && joined.script_pubkey[2] == blvm_protocol::opcodes::PUSH_20_BYTES
                        && joined.script_pubkey[23] == blvm_protocol::opcodes::OP_EQUALVERIFY
                        && joined.script_pubkey[24] == blvm_protocol::opcodes::OP_CHECKSIG
                    {
                        // OP_CHECKSIG
                        println!("  💡 This is a P2PKH script");
                        println!("  Script sig should contain: <signature> <pubkey>");
                        if input.script_sig.len() >= 2 {
                            println!(
                                "  Script sig length suggests {} signature(s) and {} pubkey(s)",
                                (input.script_sig.len() - 1) / 65,
                                1
                            );
                        }
                    }
                }
            }
            Err(e) => {
                println!("  ❌ BLVM verification: ERROR");
                println!("  Error: {:?}", e);
            }
        }

        // Verify with Bitcoin Core
        println!("");
        println!("🔐 Verifying with Bitcoin Core...");
        let rpc_client = RemoteCoreRpcClient::new();

        match rpc_client.get_block_hash(block_height).await {
            Ok(block_hash) => {
                println!("  Block hash: {}", block_hash);

                // Serialize our transaction
                let tx_hex = hex::encode(serialize_transaction(tx));

                // Test with testmempoolaccept
                match rpc_client.test_mempool_accept(&tx_hex).await {
                    Ok(result) => {
                        println!("  Bitcoin Core testmempoolaccept:");
                        if let Some(arr) = result.as_array() {
                            if let Some(first) = arr.get(0) {
                                if let Some(allowed) = first.get("allowed") {
                                    if allowed.as_bool().unwrap_or(false) {
                                        println!("  ✅ Bitcoin Core would ACCEPT this transaction");
                                        println!("  ❌ BLVM REJECTED it");
                                        println!("  🐛 THIS IS A CONSENSUS BUG!");
                                    } else {
                                        if let Some(reason) = first.get("reject-reason") {
                                            println!("  ❌ Bitcoin Core would REJECT: {}", reason);
                                            println!("  ✅ BLVM also rejected - behavior matches");
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        println!("  ⚠️  Error testing with Bitcoin Core: {}", e);
                    }
                }
            }
            Err(e) => {
                println!(
                    "  ⚠️  Block {} not found in Bitcoin Core: {}",
                    block_height, e
                );
            }
        }
    } else {
        println!("  ❌ Prevout not found in joined_sorted.bin");
        println!("  💡 This is a missing prevout (expected for unmatched inputs)");
    }

    Ok(())
}
