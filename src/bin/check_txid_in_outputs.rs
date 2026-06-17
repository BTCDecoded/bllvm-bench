//! Check if a specific txid exists in the outputs file

use anyhow::Result;
use blvm_bench::sort_merge::output_refs::OutputRef;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <txid_hex>", args[0]);
        std::process::exit(1);
    }

    let txid_hex = &args[1];
    let mut txid = [0u8; 32];
    hex::decode_to_slice(txid_hex, &mut txid)?;

    let chunks_dir = blvm_bench::require_block_cache_dir()?
        .to_string_lossy()
        .into_owned();
    let chunks_dir = std::path::PathBuf::from(chunks_dir);

    let outputs_sorted = chunks_dir.join("sort_merge_data/outputs_sorted.bin");

    println!("Searching for txid: {}", txid_hex);
    println!("In file: {}", outputs_sorted.display());

    let mut reader = BufReader::with_capacity(32 * 1024 * 1024, File::open(&outputs_sorted)?);
    let mut buf = vec![0u8; 256 * 1024];
    let mut leftover = Vec::new();
    let mut found = false;
    let mut count = 0u64;

    // Helper to read next output
    let mut read_next_output = |reader: &mut BufReader<File>,
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

    // Binary search would be better, but linear search is simpler for now
    // Since outputs are sorted, we can stop early if we pass the txid
    while let Some(output) = read_next_output(&mut reader, &mut leftover, &mut buf)? {
        count += 1;

        match output.txid.cmp(&txid) {
            std::cmp::Ordering::Equal => {
                println!("\n✅ FOUND!");
                println!("  Block: {}", output.block_height);
                println!("  Output idx: {}", output.output_idx);
                println!("  Is coinbase: {}", output.is_coinbase);
                println!("  Value: {} sats", output.value);
                found = true;
                // Continue to find all outputs with this txid
            }
            std::cmp::Ordering::Greater => {
                // We've passed where this txid would be (outputs are sorted)
                break;
            }
            std::cmp::Ordering::Less => {
                // Keep searching
            }
        }

        if count % 10_000_000 == 0 {
            println!("  Searched {} outputs...", count);
        }
    }

    if !found {
        println!("\n❌ NOT FOUND");
        println!("  Searched {} outputs total", count);
        println!("  This txid does not exist in the outputs file");
    }

    Ok(())
}
