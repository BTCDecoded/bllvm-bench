//! Quick check: sample a few missing prevout txids and see if they exist in outputs file
//! Uses a faster approach - just samples from different parts of the file

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;

use blvm_bench::sort_merge::output_refs::OutputRef;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: sample_check_prevouts <txid1> [txid2] [txid3] ...");
        eprintln!("  Each txid should be 64 hex chars (little-endian as stored)");
        return Ok(());
    }

    let outputs_file =
        blvm_bench::block_cache_env::sort_merge_data_dir()?.join("outputs_sorted.bin");

    println!(
        "Checking {} txids in: {}",
        args.len() - 1,
        outputs_file.display()
    );

    let file = File::open(&outputs_file)
        .with_context(|| format!("Failed to open: {}", outputs_file.display()))?;
    let file_size = file.metadata()?.len();
    let mut reader = BufReader::with_capacity(16 * 1024 * 1024, file);

    println!("File size: {:.2} GB\n", file_size as f64 / 1_073_741_824.0);

    // Parse txids
    let mut txids: Vec<[u8; 32]> = Vec::new();
    for i in 1..args.len() {
        let txid_hex = &args[i];
        if txid_hex.len() != 64 {
            eprintln!("Skipping invalid txid (must be 64 hex chars): {}", txid_hex);
            continue;
        }
        let mut txid = [0u8; 32];
        if let Err(e) = hex::decode_to_slice(txid_hex, &mut txid) {
            eprintln!("Skipping invalid hex txid {}: {}", txid_hex, e);
            continue;
        }
        txids.push(txid);
    }

    // For each txid, do a binary search approach
    // Since file is sorted, we can sample key positions
    for (idx, target_txid) in txids.iter().enumerate() {
        println!("{}. Checking txid: {}", idx + 1, hex::encode(target_txid));

        // Binary search approach - sample different positions
        let mut low = 0u64;
        let mut high = file_size;
        let mut found = false;
        let mut samples_checked = 0;
        let mut last_txid: Option<[u8; 32]> = None;

        // Binary search with sampling
        while low < high && samples_checked < 100 {
            let mid = (low + high) / 2;

            // Try to find a record boundary near mid
            // Read a larger chunk to find a valid record
            let mut sample_pos = mid.saturating_sub(1000);
            reader.seek(SeekFrom::Start(sample_pos))?;

            let mut buffer = vec![0u8; 2000];
            let bytes_read = reader.read(&mut buffer)?;

            // Try to parse records from this buffer
            let mut offset = 0;
            let mut sample_txid: Option<[u8; 32]> = None;

            while offset < bytes_read.saturating_sub(51) {
                if let Some((output_ref, record_size)) = OutputRef::from_bytes(&buffer[offset..]) {
                    sample_txid = Some(output_ref.txid);
                    last_txid = sample_txid;

                    match output_ref.txid.cmp(target_txid) {
                        std::cmp::Ordering::Equal => {
                            found = true;
                            println!(
                                "  ✅ FOUND! Block: {}, Output index: {}, Value: {} sat",
                                output_ref.block_height, output_ref.output_idx, output_ref.value
                            );
                            break;
                        }
                        std::cmp::Ordering::Less => {
                            // Sample is before target, search higher
                            low = sample_pos + offset as u64 + record_size as u64;
                        }
                        std::cmp::Ordering::Greater => {
                            // Sample is after target, search lower
                            high = sample_pos + offset as u64;
                            break;
                        }
                    }

                    offset += record_size;
                } else {
                    offset += 51; // Skip ahead by header size
                }
            }

            samples_checked += 1;

            if found {
                break;
            }

            if sample_txid.is_none() {
                // Couldn't parse, try different position
                low = mid + 1000;
            }
        }

        if !found {
            println!("  ❌ NOT FOUND in outputs file");
            if let Some(last) = last_txid {
                println!("     (Last checked txid: {})", hex::encode(&last));
            }
            println!("     This means:");
            println!("       - Transaction creating this output wasn't in blocks 0-912723, OR");
            println!("       - Step 3 didn't extract it, OR");
            println!("       - Txid calculation mismatch between step 1 and step 3");
        }
        println!();
    }

    Ok(())
}
