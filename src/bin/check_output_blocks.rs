//! Check what block range is actually in the outputs file

use anyhow::Result;
use blvm_bench::sort_merge::output_refs::OutputRef;
use std::fs::File;
use std::io::{BufReader, Read};

fn main() -> Result<()> {
    let chunks_dir = blvm_bench::require_block_cache_dir()?
        .to_string_lossy()
        .into_owned();
    let chunks_dir = std::path::PathBuf::from(chunks_dir);

    let outputs_sorted = chunks_dir.join("sort_merge_data/outputs_sorted.bin");

    println!("Checking block range in outputs file...");
    println!("File: {}", outputs_sorted.display());

    let mut reader = BufReader::with_capacity(32 * 1024 * 1024, File::open(&outputs_sorted)?);
    let mut buf = vec![0u8; 256 * 1024];
    let mut leftover = Vec::new();

    let mut min_block = u32::MAX;
    let mut max_block = 0u32;
    let mut count = 0u64;
    let mut last_report = std::time::Instant::now();

    // Helper to read next output
    let mut read_next = |reader: &mut BufReader<File>,
                         leftover: &mut Vec<u8>,
                         buf: &mut [u8]|
     -> Result<Option<OutputRef>> {
        loop {
            if leftover.len() >= 51 {
                if let Some((output, consumed)) = OutputRef::from_bytes(leftover) {
                    leftover.drain(..consumed);
                    return Ok(Some(output));
                }
            }

            let n = reader.read(buf)?;
            if n == 0 {
                return Ok(None);
            }
            leftover.extend_from_slice(&buf[..n]);
        }
    };

    while let Some(output) = read_next(&mut reader, &mut leftover, &mut buf)? {
        count += 1;
        min_block = min_block.min(output.block_height);
        max_block = max_block.max(output.block_height);

        if count % 10_000_000 == 0 || last_report.elapsed().as_secs() >= 5 {
            println!(
                "  Processed {} outputs, block range: {} - {}",
                count, min_block, max_block
            );
            last_report = std::time::Instant::now();
        }
    }

    println!("\nFinal results:");
    println!("  Total outputs: {}", count);
    println!("  Block range: {} - {}", min_block, max_block);
    println!("  Expected range: 0 - 912723");

    if max_block < 912723 {
        println!("\n❌ OUTPUTS FILE IS INCOMPLETE!");
        println!(
            "   Missing blocks: {} - 912723 ({} blocks)",
            max_block + 1,
            912723 - max_block
        );
    } else {
        println!("\n✅ Outputs file contains all expected blocks");
    }

    Ok(())
}
