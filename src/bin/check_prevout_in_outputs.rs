//! Diagnostic tool to check if a specific prevout txid exists in the outputs file
//! This helps determine if missing prevouts are due to extraction issues or txid mismatch
//! Uses binary search since the file is sorted by (txid, output_idx)

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;

use blvm_bench::sort_merge::output_refs::OutputRef;

fn read_record_at_offset(reader: &mut BufReader<File>, offset: u64) -> Result<Option<OutputRef>> {
    reader.seek(SeekFrom::Start(offset))?;

    // Read enough bytes for header + typical script (large scripts need more via from_bytes retry)
    let mut header = vec![0u8; 100];
    reader.read_exact(&mut header)?;

    if let Some((output_ref, _)) = OutputRef::from_bytes(&header) {
        Ok(Some(output_ref))
    } else {
        // Try reading more bytes
        reader.seek(SeekFrom::Start(offset))?;
        let mut larger = vec![0u8; 500];
        reader.read_exact(&mut larger)?;
        Ok(OutputRef::from_bytes(&larger).map(|(r, _)| r))
    }
}

fn find_record_by_txid(
    reader: &mut BufReader<File>,
    file_size: u64,
    target_txid: [u8; 32],
) -> Result<Vec<OutputRef>> {
    let mut matches = Vec::new();
    let mut low = 0u64;
    let mut high = file_size;

    // Binary search for first occurrence
    let mut first_offset = None;

    while low < high {
        let mid = (low + high) / 2;

        // Try to find a record boundary near mid
        // Search backwards for a valid record start
        let mut search_offset = mid;
        let mut found_record = false;

        for _i in 0..1000 {
            if search_offset < 100 {
                search_offset = 0;
            } else {
                search_offset -= 100;
            }

            if let Ok(Some(record)) = read_record_at_offset(reader, search_offset) {
                found_record = true;
                match record.txid.cmp(&target_txid) {
                    std::cmp::Ordering::Equal => {
                        first_offset = Some(search_offset);
                        break;
                    }
                    std::cmp::Ordering::Less => {
                        low = search_offset + 1;
                        break;
                    }
                    std::cmp::Ordering::Greater => {
                        high = search_offset;
                        break;
                    }
                }
            }
        }

        if !found_record {
            // Can't parse records in this region, try different approach
            low = mid + 1;
        }
    }

    if let Some(offset) = first_offset {
        // Found first match, now collect all matches (they're consecutive since sorted)
        let mut current_offset = offset;
        loop {
            if let Ok(Some(record)) = read_record_at_offset(reader, current_offset) {
                if record.txid == target_txid {
                    matches.push(record);
                    // Estimate next record offset (rough estimate: average record size ~100 bytes)
                    // Actually, we need to read the full record to know its size
                    current_offset += 100; // Approximation
                } else {
                    break; // No more matches
                }
            } else {
                break;
            }
        }
    }

    Ok(matches)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: check_prevout_in_outputs <txid_hex> [outputs_file]");
        eprintln!("  txid_hex: Transaction ID in hex (64 chars, little-endian as stored)");
        eprintln!("  outputs_file: Path to outputs_sorted.bin (default: from SORT_MERGE_DIR)");
        return Ok(());
    }

    let txid_hex = &args[1];
    let mut txid = [0u8; 32];

    if txid_hex.len() == 64 {
        hex::decode_to_slice(txid_hex, &mut txid).context("Failed to decode txid hex")?;
    } else {
        anyhow::bail!("Txid must be 64 hex characters");
    }

    // Determine outputs file path
    let outputs_file = if args.len() >= 3 {
        PathBuf::from(&args[2])
    } else {
        blvm_bench::block_cache_env::sort_merge_data_dir()?.join("outputs_sorted.bin")
    };

    println!("Searching for txid: {}", hex::encode(&txid));
    println!("In file: {}", outputs_file.display());

    // Open file
    let file = File::open(&outputs_file)
        .with_context(|| format!("Failed to open outputs file: {}", outputs_file.display()))?;
    let file_size = file.metadata()?.len();
    println!("File size: {:.2} GB", file_size as f64 / 1_073_741_824.0);

    let mut reader = BufReader::with_capacity(64 * 1024 * 1024, file);

    // For now, do a simpler linear search with progress
    // Binary search on variable-length records is complex
    println!("\nScanning file (this may take a while for large files)...");

    let mut matches = Vec::new();
    let mut total_records = 0u64;
    let mut buffer = vec![0u8; 1024 * 1024]; // 1MB buffer

    // Read file in chunks
    loop {
        let bytes_read = match reader.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(e) => {
                eprintln!("Read error: {}", e);
                break;
            }
        };

        // Process records in buffer
        let mut offset = 0;
        while offset < bytes_read {
            if bytes_read - offset < 51 {
                // Not enough for header, break to read more
                break;
            }

            if let Some((output_ref, record_size)) = OutputRef::from_bytes(&buffer[offset..]) {
                let current_txid = output_ref.txid;

                if current_txid == txid {
                    matches.push(output_ref);
                } else if current_txid > txid && !matches.is_empty() {
                    // We've passed all matches (file is sorted), we're done
                    println!("  Passed target txid, found {} matches", matches.len());
                    break;
                }

                offset += record_size;
                total_records += 1;

                if total_records % 10_000_000 == 0 {
                    println!(
                        "  Scanned {}M records... (current: {})",
                        total_records / 1_000_000,
                        hex::encode(&current_txid)
                    );
                }
            } else {
                // Can't parse, skip ahead
                offset += 51; // Skip at least header size
            }
        }

        if !matches.is_empty() && offset < bytes_read {
            // Check if we've passed the target
            if let Some((output_ref, _)) = OutputRef::from_bytes(&buffer[offset..]) {
                if output_ref.txid > txid {
                    break;
                }
            }
        }
    }

    println!("\nResults:");
    println!("  Total records scanned: {}", total_records);
    println!("  Matches found: {}", matches.len());

    if !matches.is_empty() {
        println!("\n✅ Found {} outputs with this txid:", matches.len());
        for (i, output) in matches.iter().take(10).enumerate() {
            println!(
                "  Output {}: index {}, block {}, value {} sat, coinbase: {}",
                i + 1,
                output.output_idx,
                output.block_height,
                output.value,
                output.is_coinbase
            );
        }
        if matches.len() > 10 {
            println!("  ... and {} more", matches.len() - 10);
        }
        println!("\n✅ Txid EXISTS in outputs file");
        println!("   This means the output WAS extracted in step 3.");
        println!("   The missing prevout is likely due to a merge-join or sorting issue.");
    } else {
        println!("\n❌ Txid NOT FOUND in outputs file");
        println!("\nPossible causes:");
        println!("  1. The transaction that created this output wasn't processed in step 3");
        println!("     (transaction is from outside block range 0-912723)");
        println!("  2. There's a txid calculation mismatch between step 1 and step 3");
        println!("     (step 1 uses prevout.hash, step 3 calculates txid)");
        println!("  3. The outputs file is incomplete (step 3 didn't finish)");
    }

    Ok(())
}
