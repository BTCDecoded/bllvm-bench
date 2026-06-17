//! Find the last output in the sorted outputs file

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

    println!("Finding last output in: {}", outputs_sorted.display());

    let file = File::open(&outputs_sorted)?;
    let file_size = file.metadata()?.len();
    let mut reader = BufReader::with_capacity(32 * 1024 * 1024, file);

    // Read last 10MB to find the last output
    let read_size = std::cmp::min(10 * 1024 * 1024, file_size);
    reader.seek(SeekFrom::End(-(read_size as i64)))?;

    let mut end_data = vec![0u8; read_size as usize];
    reader.read_exact(&mut end_data)?;

    // Parse outputs from the end data
    // We need to find the last complete output record
    let mut last_output: Option<OutputRef> = None;
    let mut pos = end_data.len();

    // Try to parse backwards from the end
    // OutputRef::from_bytes needs at least 51 bytes, then script_len
    while pos >= 51 {
        // Try to parse from this position
        if let Some((output, consumed)) =
            OutputRef::from_bytes(&end_data[pos.saturating_sub(1000)..])
        {
            // Found a valid output, but we need to find the LAST one
            // Since we're reading from near the end, the last valid parse is the last output
            if last_output.is_none() || output.txid > last_output.as_ref().unwrap().txid {
                last_output = Some(output);
            }
        }
        pos = pos.saturating_sub(100);
    }

    // Actually, better approach: read forwards from near the end and get the last one
    let mut reader2 = BufReader::with_capacity(32 * 1024 * 1024, File::open(&outputs_sorted)?);
    reader2.seek(SeekFrom::End(-(read_size as i64)))?;

    let mut buf = vec![0u8; 256 * 1024];
    let mut leftover = Vec::new();
    let mut last: Option<OutputRef> = None;

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

    // Read all outputs from near the end
    while let Some(output) = read_next(&mut reader2, &mut leftover, &mut buf)? {
        last = Some(output);
    }

    if let Some(output) = last {
        println!("\nLast output in file:");
        println!("  Txid: {}", hex::encode(output.txid));
        println!("  Block: {}", output.block_height);
        println!("  Output idx: {}", output.output_idx);
        println!("  Is coinbase: {}", output.is_coinbase);
    } else {
        println!("\nCould not find last output");
    }

    Ok(())
}
