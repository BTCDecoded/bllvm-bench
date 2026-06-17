//! Quick tool to investigate script verification failures
//!
//! Usage: investigate_failure <block_height> <tx_idx> <input_idx>

use anyhow::{Context, Result};
use blvm_protocol::Witness;
use blvm_protocol::block::calculate_script_flags_for_block_network;
use blvm_protocol::script::{SigVersion, verify_script_with_context_full};
use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
use blvm_protocol::transaction::is_coinbase;
use blvm_protocol::types::{Network, TransactionOutput};

use blvm_bench::chunked_cache::ChunkedBlockIterator;
use blvm_bench::sort_merge::verify::PrevoutReader;
use blvm_protocol::witness::is_witness_empty;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: investigate_failure <block_height> <tx_idx> <input_idx>");
        eprintln!("Example: investigate_failure 382188 106 3");
        std::process::exit(1);
    }

    let block_height: u64 = args[1].parse().context("Invalid block height")?;
    let tx_idx: usize = args[2].parse().context("Invalid tx index")?;
    let input_idx: usize = args[3].parse().context("Invalid input index")?;

    println!("Investigating failure:");
    println!("  Block: {}", block_height);
    println!("  Transaction index: {}", tx_idx);
    println!("  Input index: {}", input_idx);

    // Load block
    let chunks_dir = blvm_bench::require_block_cache_dir()?;
    let mut block_iter = ChunkedBlockIterator::new(&chunks_dir, Some(block_height), Some(1))?
        .ok_or_else(|| anyhow::anyhow!("Failed to create block iterator"))?;

    let block_data = block_iter
        .next_block()?
        .ok_or_else(|| anyhow::anyhow!("Block {} not found", block_height))?;

    let (block, witnesses) =
        deserialize_block_with_witnesses(&block_data).context("Failed to deserialize block")?;

    println!(
        "\n✅ Loaded block {} ({} transactions)",
        block_height,
        block.transactions.len()
    );

    if tx_idx >= block.transactions.len() {
        anyhow::bail!(
            "Transaction index {} out of range (block has {} transactions)",
            tx_idx,
            block.transactions.len()
        );
    }

    let tx = &block.transactions[tx_idx];
    println!("\n📋 Transaction {}:", tx_idx);
    println!("  Is coinbase: {}", is_coinbase(tx));
    println!("  Inputs: {}", tx.inputs.len());
    println!("  Outputs: {}", tx.outputs.len());

    if is_coinbase(tx) {
        println!(
            "\n⚠️  This is a coinbase transaction - coinbase inputs don't have scripts to verify!"
        );
        return Ok(());
    }

    if input_idx >= tx.inputs.len() {
        anyhow::bail!(
            "Input index {} out of range (tx has {} inputs)",
            input_idx,
            tx.inputs.len()
        );
    }

    let input = &tx.inputs[input_idx];
    println!("\n🔍 Input {}:", input_idx);
    println!("  Prevout txid: {}", hex::encode(&input.prevout.hash));
    println!("  Prevout index: {}", input.prevout.index);
    println!(
        "  Script sig ({} bytes): {}",
        input.script_sig.len(),
        hex::encode(&input.script_sig)
    );

    // Load prevouts for this block using PrevoutReader (efficient skip)
    let prevouts_file =
        blvm_bench::block_cache_env::sort_merge_data_dir()?.join("joined_sorted.bin");
    let mut prevout_reader = PrevoutReader::new(&prevouts_file)?;

    // Skip to the block
    prevout_reader.skip_to_block(block_height as u32)?;

    // Read all prevouts for this block
    let block_prevouts = prevout_reader.read_block_prevouts(block_height as u32)?;

    println!("\n🔍 Found {} prevouts for block", block_prevouts.len());

    // Find the specific prevout
    let prevout = block_prevouts
        .iter()
        .find(|p| p.spending_tx_idx == tx_idx as u32 && p.spending_input_idx == input_idx as u32)
        .ok_or_else(|| {
            anyhow::anyhow!("Prevout not found for tx {}, input {}", tx_idx, input_idx)
        })?;

    println!("✅ Found prevout:");
    println!("  Value: {} satoshis", prevout.value);
    println!(
        "  Script pubkey ({} bytes): {}",
        prevout.script_pubkey.len(),
        hex::encode(&prevout.script_pubkey)
    );
    println!("  Prevout height: {}", prevout.prevout_height);
    println!("  Is coinbase: {}", prevout.is_coinbase);

    // Get witness
    let tx_witness = witnesses.get(tx_idx);
    let witness_stack: Option<&Witness> = tx_witness.and_then(|w| w.get(input_idx));

    if let Some(witness) = witness_stack {
        println!("\n📝 Witness stack ({} elements):", witness.len());
        for (i, elem) in witness.iter().enumerate() {
            println!("  [{}] {} bytes: {}", i, elem.len(), hex::encode(elem));
        }
    } else {
        println!("\n📝 No witness data for this input");
    }

    let wits = witnesses.get(tx_idx).map(|w| w.as_slice()).unwrap_or(&[]);
    let has_witness = wits.iter().any(|wit| !is_witness_empty(wit));
    let flags =
        calculate_script_flags_for_block_network(tx, has_witness, block_height, Network::Mainnet);
    println!("\n🏳️  Script flags: 0x{:x}", flags);
    println!("  P2SH: {}", (flags & 0x01) != 0);
    println!("  DERSIG: {}", (flags & 0x04) != 0);
    println!("  CHECKLOCKTIMEVERIFY: {}", (flags & 0x200) != 0);
    println!("  CHECKSEQUENCEVERIFY: {}", (flags & 0x400) != 0);
    println!("  WITNESS: {}", (flags & 0x800) != 0);
    println!("  NULLDUMMY: {}", (flags & 0x10) != 0);
    println!(
        "  WITNESS_PUBKEYTYPE (Taproot outputs): {}",
        (flags & 0x8000) != 0
    );

    // Build all prevouts for this transaction from block_prevouts
    println!("\n🔍 Building prevouts for transaction...");
    let mut all_prevouts = Vec::new();
    for i in 0..tx.inputs.len() {
        let prevout_opt = block_prevouts
            .iter()
            .find(|p| p.spending_tx_idx == tx_idx as u32 && p.spending_input_idx == i as u32);
        all_prevouts.push(
            prevout_opt
                .map(|p| TransactionOutput {
                    value: p.value,
                    script_pubkey: p.script_pubkey.clone(),
                })
                .unwrap_or_else(|| TransactionOutput {
                    value: 0,
                    script_pubkey: vec![],
                }),
        );
    }

    println!("  Built {} prevouts", all_prevouts.len());

    // Verify script
    println!("\n🔐 Verifying script...");
    let prevout_values: Vec<i64> = all_prevouts.iter().map(|o| o.value).collect();
    let prevout_script_pubkeys: Vec<&[u8]> = all_prevouts
        .iter()
        .map(|o| o.script_pubkey.as_slice())
        .collect();

    match verify_script_with_context_full(
        &input.script_sig,
        &prevout.script_pubkey,
        witness_stack,
        flags,
        tx,
        input_idx,
        &prevout_values,
        &prevout_script_pubkeys,
        Some(block_height),
        None, // median_time_past
        Network::Mainnet,
        SigVersion::Base,
        None,
        None,
        None,
        None,
        None,
    ) {
        Ok(true) => {
            println!("✅ Script verification PASSED");
            println!("⚠️  But the test said it failed - this is strange!");
        }
        Ok(false) => {
            println!("❌ Script verification FAILED (returned false)");
            println!("🔍 This means BLVM rejected a script from a mainnet block!");
            println!("⚠️  If another full node accepted this block, this may be a CONSENSUS BUG!");
        }
        Err(e) => {
            println!("❌ Script verification ERROR: {:?}", e);
            println!("🔍 This could be a bug in script verification!");
        }
    }

    Ok(())
}
