//! Inspect the merge-join issue: why did outputs run out?
//!
//! This tool will:
//! 1. Read the last N outputs from outputs_sorted.bin
//! 2. Read the last N inputs from inputs_sorted.bin (the unmatched ones)
//! 3. Compare their txids to understand what's happening

use anyhow::Result;
use blvm_bench::sort_merge::input_refs::InputRef;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

fn main() -> Result<()> {
    let chunks_dir = blvm_bench::require_block_cache_dir()?
        .to_string_lossy()
        .into_owned();
    let chunks_dir = std::path::PathBuf::from(chunks_dir);

    let data_dir = chunks_dir.join("sort_merge_data");
    let inputs_sorted = data_dir.join("inputs_sorted.bin");
    let outputs_sorted = data_dir.join("outputs_sorted.bin");

    println!("Inspecting merge-join issue...");
    println!("Inputs: {}", inputs_sorted.display());
    println!("Outputs: {}", outputs_sorted.display());

    // Get file sizes
    let input_size = std::fs::metadata(&inputs_sorted)?.len();
    let output_size = std::fs::metadata(&outputs_sorted)?.len();

    let input_records = input_size / InputRef::SIZE as u64;
    let mut output_reader = BufReader::new(File::open(&outputs_sorted)?);

    println!("\nFile sizes:");
    println!("  Inputs: {} records ({} bytes)", input_records, input_size);
    println!("  Outputs: {} bytes", output_size);

    // For now, focus on inputs - we'll check outputs separately

    // Better approach: read last 1000 inputs (the unmatched ones)
    println!("\nReading last 1000 inputs (likely unmatched)...");
    let mut input_file = File::open(&inputs_sorted)?;
    let skip = (input_records.saturating_sub(1000)) * InputRef::SIZE as u64;
    input_file.seek(SeekFrom::Start(skip))?;

    let mut last_inputs = Vec::new();
    let mut buf = [0u8; InputRef::SIZE];
    for _ in 0..1000 {
        match input_file.read_exact(&mut buf) {
            Ok(_) => {
                last_inputs.push(InputRef::from_bytes(&buf));
            }
            Err(_) => break,
        }
    }

    println!("\nLast 10 inputs (likely unmatched):");
    for (i, input) in last_inputs.iter().rev().take(10).enumerate() {
        println!("\nInput {}:", i + 1);
        println!("  Spending block: {}", input.block_height);
        println!("  Prevout txid: {}", hex::encode(input.prevout_txid));
        println!("  Prevout idx: {}", input.prevout_idx);
    }

    // Now try to find these txids in the outputs file
    // This is expensive, so let's just check if they're in a reasonable range
    println!("\nAnalysis:");
    println!("  These inputs have prevout_txid that should match outputs");
    println!("  If outputs ran out, it means these txids are > all output txids");
    println!("  This could mean:");
    println!("    1. These inputs reference outputs from blocks > 912,723");
    println!("    2. There's a sorting issue");
    println!("    3. The outputs file is incomplete");

    Ok(())
}
