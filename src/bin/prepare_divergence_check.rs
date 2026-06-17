//! Pre-process failures.log to add TX hex for efficient divergence checking
//! Processes chunks in order for optimal I/O performance

use anyhow::Result;
use blvm_bench::chunk_index::{BlockIndex, load_block_index};
use blvm_bench::chunked_cache::SharedChunkCache;
use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
use blvm_protocol::serialization::transaction::serialize_transaction;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
struct FailureInfo {
    block_height: u64,
    tx_idx: usize,
    input_idx: usize,
    error_type: String,
    original_line: String,
}

fn get_chunk_for_height(index: &BlockIndex, height: u64) -> Option<usize> {
    index.get(&height).map(|e| e.chunk_number)
}

fn main() -> Result<()> {
    let sm = blvm_bench::block_cache_env::sort_merge_data_dir()?;
    let failures_file = sm.join("failures.log");
    let output_file = sm.join("failures_with_hex.log");
    let chunks_dir = blvm_bench::require_block_cache_dir()?;

    println!("📦 Preparing failures.log with TX hex...");
    println!();

    // Load block index
    println!("Loading block index...");
    let block_index =
        Arc::new(load_block_index(&chunks_dir)?.ok_or_else(|| anyhow::anyhow!("No index"))?);
    println!("  ✅ Block index ({} entries)", block_index.len());

    // Parse failures and group by chunk
    println!("\nParsing failures...");
    let file = File::open(&failures_file)?;
    let reader = BufReader::new(file);

    let mut failures_by_chunk: BTreeMap<usize, Vec<FailureInfo>> = BTreeMap::new();
    let mut total = 0usize;
    let mut script_failures = 0usize;

    for line in reader.lines().filter_map(|l| l.ok()) {
        if line.starts_with('#') {
            continue;
        }
        total += 1;

        // Only process "Script returned false" failures (not missing prevouts)
        if !line.contains("Script returned false") {
            continue;
        }
        script_failures += 1;

        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 3 {
            continue;
        }

        let block_height: u64 = match parts[0].trim().parse() {
            Ok(h) if h > 0 => h,
            _ => continue,
        };

        let error_type = parts[1].trim().to_string();
        let msg = parts[2].trim();

        let tx_idx: usize = match msg.find("tx ").and_then(|s| {
            msg[s + 3..]
                .find(',')
                .and_then(|e| msg[s + 3..s + 3 + e].trim().parse().ok())
        }) {
            Some(t) => t,
            None => continue,
        };

        let input_idx: usize = msg
            .find("input ")
            .and_then(|s| msg[s + 6..].trim().parse().ok())
            .unwrap_or(0);

        let chunk = match get_chunk_for_height(&block_index, block_height) {
            Some(c) => c,
            None => continue,
        };

        failures_by_chunk
            .entry(chunk)
            .or_default()
            .push(FailureInfo {
                block_height,
                tx_idx,
                input_idx,
                error_type,
                original_line: line.clone(),
            });
    }

    println!("  Total lines: {}", total);
    println!("  Script failures: {}", script_failures);
    println!("  Grouped into {} chunks:", failures_by_chunk.len());
    for (chunk, failures) in &failures_by_chunk {
        println!("    Chunk {}: {} failures", chunk, failures.len());
    }

    // Open output file
    let out_file = File::create(&output_file)?;
    let mut writer = BufWriter::new(out_file);
    writeln!(writer, "# Block Height | Error Type | Details | TX Hex")?;

    let start = Instant::now();
    let mut processed = 0usize;
    let mut written = 0usize;

    println!("\n================================================================================");
    println!("Processing chunks in order...\n");

    // Process each chunk in order
    for (chunk_num, failures) in &failures_by_chunk {
        println!("📦 Chunk {} - {} failures", chunk_num, failures.len());
        let chunk_start = Instant::now();

        // Create cache for this chunk
        let chunk_cache = SharedChunkCache::new(&chunks_dir, Arc::clone(&block_index));

        // Group by block for efficient loading
        let mut by_block: HashMap<u64, Vec<&FailureInfo>> = HashMap::new();
        for f in failures {
            by_block.entry(f.block_height).or_default().push(f);
        }

        // Sort blocks
        let mut block_heights: Vec<u64> = by_block.keys().copied().collect();
        block_heights.sort();

        let mut chunk_written = 0usize;

        for block_height in block_heights {
            let block_failures = &by_block[&block_height];

            // Load block
            let data = match chunk_cache.load_block(block_height) {
                Ok(Some(d)) => d,
                _ => {
                    eprintln!("    ⚠️  Could not load block {}", block_height);
                    continue;
                }
            };
            let (block, _) = match deserialize_block_with_witnesses(&data) {
                Ok(b) => b,
                _ => continue,
            };

            // Write each failure with tx hex
            for failure in block_failures {
                processed += 1;

                if failure.tx_idx >= block.transactions.len() {
                    eprintln!(
                        "    ⚠️  Invalid tx_idx {} for block {} ({} txs)",
                        failure.tx_idx,
                        block_height,
                        block.transactions.len()
                    );
                    continue;
                }

                let tx = &block.transactions[failure.tx_idx];
                let tx_hex = hex::encode(serialize_transaction(tx));

                writeln!(
                    writer,
                    "{} | {} | Script returned false: tx {}, input {} | {}",
                    failure.block_height,
                    failure.error_type,
                    failure.tx_idx,
                    failure.input_idx,
                    tx_hex
                )?;

                written += 1;
                chunk_written += 1;
            }

            // Progress dots
            if chunk_written % 100 == 0 {
                print!(".");
                let _ = std::io::stdout().flush();
            }
        }

        let chunk_time = chunk_start.elapsed();
        println!(
            "\n  ✅ {} txs in {:.1}s ({:.1} tx/s)\n",
            chunk_written,
            chunk_time.as_secs_f64(),
            chunk_written as f64 / chunk_time.as_secs_f64().max(0.1)
        );
    }

    writer.flush()?;

    let elapsed = start.elapsed();

    println!("================================================================================\n");
    println!(
        "DONE ({:.1}s = {:.1}m)",
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() / 60.0
    );
    println!("  Processed: {}", processed);
    println!("  Written: {}", written);
    println!("  Output: {}", output_file.display());
    println!("\nNow run: check_divergences (it will use failures_with_hex.log)");

    Ok(())
}
