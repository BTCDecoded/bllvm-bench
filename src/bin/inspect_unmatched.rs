//! Inspect unmatched inputs from merge-join to understand why they weren't matched

use anyhow::Result;
use blvm_bench::sort_merge::input_refs::InputRef;
use blvm_bench::sort_merge::output_refs::OutputRef;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

fn main() -> Result<()> {
    let chunks_dir = blvm_bench::require_block_cache_dir()?
        .to_string_lossy()
        .into_owned();
    let chunks_dir = std::path::PathBuf::from(chunks_dir);

    let data_dir = chunks_dir.join("sort_merge_data");
    let inputs_sorted = data_dir.join("inputs_sorted.bin");
    let outputs_sorted = data_dir.join("outputs_sorted.bin");

    println!("Inspecting unmatched inputs...");
    println!("Inputs: {}", inputs_sorted.display());
    println!("Outputs: {}", outputs_sorted.display());

    // Read last portion of inputs to see what's unmatched
    let input_file = File::open(&inputs_sorted)?;
    let input_size = input_file.metadata()?.len();
    let input_records = input_size / InputRef::SIZE as u64;

    println!("\nTotal inputs: {}", input_records);
    println!("Reading last 1000 inputs to inspect unmatched ones...");

    // Read last 1000 inputs
    let mut reader = BufReader::new(input_file);
    let skip = (input_records.saturating_sub(1000)) * InputRef::SIZE as u64;
    reader.seek_relative(skip as i64)?;

    let mut last_inputs = Vec::new();
    let mut buf = [0u8; InputRef::SIZE];
    for _ in 0..1000 {
        match reader.read_exact(&mut buf) {
            Ok(_) => {
                last_inputs.push(InputRef::from_bytes(&buf));
            }
            Err(_) => break,
        }
    }

    // Read last output to see what the max txid is
    let output_file = File::open(&outputs_sorted)?;
    let output_size = output_file.metadata()?.len();
    let mut output_reader = BufReader::new(output_file);

    // Find last output by reading backwards
    let mut last_output: Option<OutputRef> = None;
    let mut output_buf = vec![0u8; 256 * 1024];
    let mut output_leftover: Vec<u8> = Vec::new();

    // Read all outputs to find the last one (simplified - just read last few)
    // For now, let's just check if inputs are greater than any output

    println!("\nLast 10 inputs (likely unmatched):");
    for (i, input) in last_inputs.iter().rev().take(10).enumerate() {
        println!("\nInput {}:", i + 1);
        println!("  Spending block: {}", input.block_height);
        println!("  Prevout txid: {}", hex::encode(input.prevout_txid));
        println!("  Prevout idx: {}", input.prevout_idx);
    }

    // Check if these inputs reference outputs from outside the range
    // Inputs are from blocks 367,854-912,723
    // Outputs should be from blocks 0-912,723
    // So if an input's prevout_txid is from a tx in block > 912,723, that's the issue

    Ok(())
}
