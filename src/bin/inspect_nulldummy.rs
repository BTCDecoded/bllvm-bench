//! Inspect the actual dummy element in a NULLDUMMY failing transaction
//! This will help us understand why BLVM rejects it

use anyhow::{Context, Result};
use blvm_bench::chunked_cache::ChunkedBlockIterator;
use blvm_protocol::block::calculate_tx_id;
use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: {} <block_height> <tx_idx> <input_idx>", args[0]);
        eprintln!(
            "Example: {} <mainnet_height_post_segwit_bip141> 168 0",
            args[0]
        );
        std::process::exit(1);
    }

    let block_height: u64 = args[1].parse()?;
    let tx_idx: usize = args[2].parse()?;
    let input_idx: usize = args[3].parse()?;

    let chunks_dir = blvm_bench::require_block_cache_dir()?;

    println!("🔍 Inspecting NULLDUMMY failure:");
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

    let tx = &block.transactions[tx_idx];
    let input = &tx.inputs[input_idx];

    println!("📋 Transaction details:");
    println!("  Transaction ID: {}", hex::encode(calculate_tx_id(tx)));
    println!("  Script sig: {}", hex::encode(&input.script_sig));
    println!("  Script sig length: {} bytes", input.script_sig.len());
    println!("");

    // Parse script_sig to find OP_CHECKMULTISIG and the dummy element
    // Stack layout: [dummy] [sig1] ... [sigm] [m] [pubkey1] ... [pubkeyn] [n]
    // OP_CHECKMULTISIG consumes: n + m + 2 elements (including dummy)
    // The dummy is the FIRST element consumed (last on stack before execution)

    if let Some(pos) = input.script_sig.iter().position(|&b| b == 0xae) {
        println!("  ✅ Found OP_CHECKMULTISIG (0xae) at position {}", pos);

        // Parse backwards from OP_CHECKMULTISIG to find the dummy element
        // The dummy is the last element pushed before OP_CHECKMULTISIG
        // We need to parse the script backwards to find it

        println!("\n🔎 Parsing script_sig to find dummy element...");
        println!("  Script bytes before OP_CHECKMULTISIG:");
        let before = &input.script_sig[..pos];
        if before.len() > 0 {
            println!("    {}", hex::encode(before));
            println!("    Length: {} bytes", before.len());

            // Try to find the last push operation
            // Parse backwards from pos
            let mut i = pos;
            let mut found_dummy = false;

            while i > 0 {
                i -= 1;
                let byte = input.script_sig[i];

                // Check if this is a push opcode
                if byte == 0x00 {
                    // OP_0 - this could be the dummy!
                    println!("    Found OP_0 (0x00) at position {}", i);
                    println!("    This is likely the dummy element!");
                    found_dummy = true;
                    break;
                } else if byte <= 0x4b {
                    // Direct push: opcode is length
                    let len = byte as usize;
                    if i >= len {
                        println!(
                            "    Found push opcode 0x{:02x} (length {}) at position {}",
                            byte, len, i
                        );
                        let data_start = i + 1;
                        let data_end = data_start + len;
                        if data_end <= pos {
                            let data = &input.script_sig[data_start..data_end];
                            println!("    Pushed data: {}", hex::encode(data));
                            if i + len + 1 == pos {
                                println!(
                                    "    ⚠️  This push is right before OP_CHECKMULTISIG - this is the dummy!"
                                );
                                println!("    Dummy element: {}", hex::encode(data));
                                if data == [0x00] {
                                    println!("    ✅ Dummy is [0x00] - this is VALID per BIP147!");
                                    println!("    ❌ But BLVM rejected it - this is a BUG!");
                                } else if data.is_empty() {
                                    println!(
                                        "    ⚠️  Dummy is [] (empty) - BIP147 requires [0x00]"
                                    );
                                } else {
                                    println!(
                                        "    ❌ Dummy is non-empty and not [0x00] - this is INVALID"
                                    );
                                }
                                found_dummy = true;
                                break;
                            }
                        }
                        if i < len {
                            break;
                        }
                        i -= len; // Skip the data
                    }
                }
            }

            if !found_dummy {
                println!("    ⚠️  Could not find dummy element by parsing backwards");
                println!("    This might require full script execution to determine");
            }
        }
    } else {
        println!("  ❌ OP_CHECKMULTISIG (0xae) not found in script_sig");
        println!("  💡 This shouldn't trigger NULLDUMMY check - investigating...");
    }

    Ok(())
}
