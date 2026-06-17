//! Quick check to verify if outputs_sorted.bin is complete
//! This is much faster than re-sorting - just checks the last few outputs

use anyhow::Result;
use blvm_bench::sort_merge::output_refs::OutputRef;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

fn main() -> Result<()> {
    let chunks_dir = blvm_bench::require_block_cache_dir()?
        .to_string_lossy()
        .into_owned();
    let chunks_dir = std::path::PathBuf::from(chunks_dir);

    let outputs_sorted = chunks_dir.join("sort_merge_data/outputs_sorted.bin");

    println!("Quick completeness check for outputs_sorted.bin...");

    let file = File::open(&outputs_sorted)?;
    let file_size = file.metadata()?.len();
    let mut reader = BufReader::with_capacity(32 * 1024 * 1024, file);

    // Read last 10MB to find last few outputs
    let read_size = std::cmp::min(10 * 1024 * 1024, file_size);
    reader.seek(SeekFrom::End(-(read_size as i64)))?;

    let mut end_data = vec![0u8; read_size as usize];
    reader.read_exact(&mut end_data)?;

    // Parse outputs from the end (simplified - just find last valid output)
    let mut last_output: Option<OutputRef> = None;
    let mut buf = vec![0u8; 256 * 1024];
    let mut leftover = Vec::new();

    // Read from the end data
    leftover.extend_from_slice(&end_data);

    // Try to parse the last output
    // Start from near the end and work backwards
    for offset in (0..end_data.len().saturating_sub(1000)).rev().step_by(100) {
        if let Some((output, _)) = OutputRef::from_bytes(&end_data[offset..]) {
            if last_output.is_none() || output.txid > last_output.as_ref().unwrap().txid {
                last_output = Some(output);
            }
        }
    }

    // Actually, better: read forwards from near the end
    let mut reader2 = BufReader::with_capacity(32 * 1024 * 1024, File::open(&outputs_sorted)?);
    reader2.seek(SeekFrom::End(-(read_size as i64)))?;

    let mut last: Option<OutputRef> = None;
    let mut leftover2 = Vec::new();

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

    while let Some(output) = read_next(&mut reader2, &mut leftover2, &mut buf)? {
        last = Some(output);
    }

    if let Some(output) = last {
        println!("\nLast output in sorted file:");
        println!("  Block: {}", output.block_height);
        println!("  Txid: {}", hex::encode(output.txid));

        if output.block_height >= 912722 {
            println!(
                "\n✅ Sorted file is COMPLETE (last block: {})",
                output.block_height
            );
        } else {
            println!("\n❌ Sorted file is INCOMPLETE!");
            println!("   Last block: {} (expected: 912722)", output.block_height);
            println!("   Missing: {} blocks", 912722 - output.block_height);
            println!("\n   Solution: Re-run step3b to sort the complete unsorted file");
        }
    } else {
        println!("\n❌ Could not read last output - file may be corrupted");
    }

    Ok(())
}
