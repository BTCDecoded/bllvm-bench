//! Step 1: Extract input references from all blocks
//!
//! For each non-coinbase input, record:
//! - prevout_txid (32 bytes): Which transaction output is being spent
//! - prevout_idx (4 bytes): Which output index
//! - block_height (4 bytes): Where this input is
//! - tx_idx (4 bytes): Which transaction in the block
//! - input_idx (4 bytes): Which input in the transaction
//!
//! Total: 48 bytes per input record
//! ~150M inputs × 48 bytes = ~7.2 GB

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
use blvm_protocol::transaction::is_coinbase;

use crate::chunked_cache::ChunkedBlockIterator;

/// Fixed-size input reference record (48 bytes)
#[derive(Debug, Clone, Copy)]
pub struct InputRef {
    /// txid of the output being spent
    pub prevout_txid: [u8; 32],
    /// index of the output being spent
    pub prevout_idx: u32,
    /// block height of the spending transaction
    pub block_height: u32,
    /// transaction index within the block
    pub tx_idx: u32,
    /// input index within the transaction
    pub input_idx: u32,
}

impl InputRef {
    pub const SIZE: usize = 48;

    pub fn to_bytes(&self) -> [u8; 48] {
        let mut buf = [0u8; 48];
        buf[0..32].copy_from_slice(&self.prevout_txid);
        buf[32..36].copy_from_slice(&self.prevout_idx.to_le_bytes());
        buf[36..40].copy_from_slice(&self.block_height.to_le_bytes());
        buf[40..44].copy_from_slice(&self.tx_idx.to_le_bytes());
        buf[44..48].copy_from_slice(&self.input_idx.to_le_bytes());
        buf
    }

    pub fn from_bytes(buf: &[u8; 48]) -> Self {
        let mut prevout_txid = [0u8; 32];
        prevout_txid.copy_from_slice(&buf[0..32]);
        Self {
            prevout_txid,
            prevout_idx: u32::from_le_bytes([buf[32], buf[33], buf[34], buf[35]]),
            block_height: u32::from_le_bytes([buf[36], buf[37], buf[38], buf[39]]),
            tx_idx: u32::from_le_bytes([buf[40], buf[41], buf[42], buf[43]]),
            input_idx: u32::from_le_bytes([buf[44], buf[45], buf[46], buf[47]]),
        }
    }
}

/// Extract all input references from blocks and write to file
pub fn extract_input_refs(
    chunks_dir: &Path,
    output_file: &Path,
    start_height: u64,
    end_height: u64,
    progress_interval: u64,
) -> Result<u64> {
    println!("\n{}", "═".repeat(60));
    println!("STEP 1: Extract Input References");
    println!("{}", "═".repeat(60));
    println!("  Chunks dir: {}", chunks_dir.display());
    println!("  Blocks: {} to {}", start_height, end_height);
    println!("  Output: {}", output_file.display());

    let start_time = Instant::now();

    // Create block iterator
    // CRITICAL FIX: Calculate max_blocks from end_height to ensure we process all requested blocks
    // Note: Iterator will still stop at metadata.total_blocks if chunks don't have all blocks,
    // but this ensures we try to process up to end_height
    let max_blocks = (end_height - start_height) as usize;
    let mut block_iter =
        ChunkedBlockIterator::new(chunks_dir, Some(start_height), Some(max_blocks))?.ok_or_else(
            || anyhow::anyhow!("Failed to create block iterator - chunks.meta not found?"),
        )?;

    // Log the actual end_height the iterator will use (may be limited by metadata.total_blocks)
    eprintln!(
        "  📍 Block iterator configured: start={}, requested_end={}, max_blocks={}",
        start_height, end_height, max_blocks
    );

    // Create output file
    let file = File::create(output_file)
        .with_context(|| format!("Failed to create output file: {}", output_file.display()))?;
    let mut writer = BufWriter::with_capacity(64 * 1024 * 1024, file); // 64MB buffer

    let mut total_inputs = 0u64;
    let mut height = start_height;
    let mut last_report = Instant::now();

    while height < end_height {
        // Get next block
        let block_data = match block_iter.next_block()? {
            Some(data) => data,
            None => {
                if height < end_height {
                    eprintln!(
                        "  ⚠️  WARNING: Block iterator ended at height {} but end_height is {}",
                        height, end_height
                    );
                    eprintln!(
                        "  Missing blocks: {} to {} ({} blocks)",
                        height,
                        end_height - 1,
                        end_height - height
                    );
                    eprintln!(
                        "  This will cause missing inputs for blocks {} to {}",
                        height,
                        end_height - 1
                    );
                    eprintln!("  Possible causes:");
                    eprintln!(
                        "    1. Chunks don't contain all blocks up to {}",
                        end_height
                    );
                    eprintln!(
                        "    2. Chunk metadata (chunks.meta) reports fewer blocks than available"
                    );
                    eprintln!("    3. Block index is incomplete");
                    eprintln!(
                        "  Solution: Ensure chunks contain all blocks up to {} or update chunks.meta",
                        end_height
                    );
                }
                // Stop extraction - we've processed all available blocks
                break;
            }
        };

        // Deserialize block
        let (block, _witnesses) = deserialize_block_with_witnesses(&block_data)
            .with_context(|| format!("Failed to deserialize block {}", height))?;

        // Process each transaction
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            // Skip coinbase inputs
            if is_coinbase(tx) {
                continue;
            }

            // Record each input
            for (input_idx, input) in tx.inputs.iter().enumerate() {
                let record = InputRef {
                    prevout_txid: input.prevout.hash,
                    prevout_idx: input.prevout.index as u32,
                    block_height: height as u32,
                    tx_idx: tx_idx as u32,
                    input_idx: input_idx as u32,
                };

                writer.write_all(&record.to_bytes())?;
                total_inputs += 1;
            }
        }

        // Progress report
        let processed = height - start_height + 1;
        if processed % progress_interval == 0 || last_report.elapsed().as_secs() >= 10 {
            let elapsed = start_time.elapsed().as_secs_f64();
            let rate = processed as f64 / elapsed;
            let remaining = (end_height - height) as f64 / rate;
            println!(
                "  Block {}/{} ({:.1}%) - {} inputs - {:.0} blk/s - ETA: {:.0}m",
                height,
                end_height,
                (height - start_height) as f64 / (end_height - start_height) as f64 * 100.0,
                total_inputs,
                rate,
                remaining / 60.0
            );
            last_report = Instant::now();
        }

        height += 1;
    }

    writer.flush()?;

    let elapsed = start_time.elapsed();
    let file_size = std::fs::metadata(output_file)?.len();

    println!("{}", "─".repeat(60));
    println!("  ✅ Step 1 Complete!");
    println!("  Total inputs: {}", total_inputs);
    println!("  Blocks processed: {}", height - start_height);
    println!("  File size: {:.2} GB", file_size as f64 / 1_073_741_824.0);
    println!("  Time: {:.1}m", elapsed.as_secs_f64() / 60.0);
    println!(
        "  Rate: {:.0} blocks/sec",
        (height - start_height) as f64 / elapsed.as_secs_f64()
    );

    Ok(total_inputs)
}

/// Sort input refs file by (prevout_txid, prevout_idx) using external merge sort
///
/// Uses a proper binary merge sort that doesn't expand data size:
/// 1. Read chunks into memory (2GB each)
/// 2. Sort each chunk in memory
/// 3. Write sorted chunks to temp files
/// 4. Multi-way merge the sorted chunks
pub fn sort_input_refs(input_file: &Path, output_file: &Path) -> Result<()> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    use std::io::{BufReader, Read, Seek, SeekFrom};

    println!("\n{}", "═".repeat(60));
    println!("STEP 2: Sort Input References by Prevout");
    println!("{}", "═".repeat(60));
    println!("  Input: {}", input_file.display());
    println!("  Output: {}", output_file.display());

    let start_time = Instant::now();

    let input_size = std::fs::metadata(input_file)?.len();
    let num_records = input_size / InputRef::SIZE as u64;
    println!(
        "  Records: {} ({:.2} GB)",
        num_records,
        input_size as f64 / 1_073_741_824.0
    );

    // Chunk size: 2GB = ~44M records
    let chunk_records = 44_000_000usize;
    let chunk_bytes = chunk_records * InputRef::SIZE;

    // Create temp directory
    let temp_dir = input_file
        .parent()
        .unwrap_or(Path::new("."))
        .join("sort_tmp");
    std::fs::create_dir_all(&temp_dir)?;

    // Phase 1: Create sorted chunks
    println!("  Phase 1: Creating sorted chunks...");
    let mut reader = BufReader::with_capacity(64 * 1024 * 1024, File::open(input_file)?);
    let mut chunk_files: Vec<std::path::PathBuf> = Vec::new();
    let mut chunk_idx = 0;

    loop {
        // Read chunk into memory
        let mut records: Vec<InputRef> = Vec::with_capacity(chunk_records);
        let mut buf = [0u8; InputRef::SIZE];

        for _ in 0..chunk_records {
            match reader.read_exact(&mut buf) {
                Ok(()) => records.push(InputRef::from_bytes(&buf)),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
        }

        if records.is_empty() {
            break;
        }

        // Sort in memory by (prevout_txid, prevout_idx)
        records.sort_unstable_by(|a, b| {
            a.prevout_txid
                .cmp(&b.prevout_txid)
                .then_with(|| a.prevout_idx.cmp(&b.prevout_idx))
        });

        // Write sorted chunk
        let chunk_path = temp_dir.join(format!("chunk_{}.bin", chunk_idx));
        let mut chunk_writer =
            BufWriter::with_capacity(64 * 1024 * 1024, File::create(&chunk_path)?);
        for record in &records {
            chunk_writer.write_all(&record.to_bytes())?;
        }
        chunk_writer.flush()?;

        println!("    Chunk {}: {} records", chunk_idx, records.len());
        chunk_files.push(chunk_path);
        chunk_idx += 1;
    }

    println!("  Phase 2: Merging {} chunks...", chunk_files.len());

    // Phase 2: K-way merge
    // For each chunk, keep a reader and current record
    struct ChunkReader {
        reader: BufReader<File>,
        current: Option<InputRef>,
        chunk_idx: usize,
    }

    impl ChunkReader {
        fn read_next(&mut self) -> Result<()> {
            let mut buf = [0u8; InputRef::SIZE];
            match self.reader.read_exact(&mut buf) {
                Ok(()) => {
                    self.current = Some(InputRef::from_bytes(&buf));
                    Ok(())
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.current = None;
                    Ok(())
                }
                Err(e) => Err(e.into()),
            }
        }
    }

    // Wrapper for heap ordering (min-heap by key)
    #[derive(Eq, PartialEq)]
    struct HeapItem {
        key: ([u8; 32], u32),
        chunk_idx: usize,
    }

    impl Ord for HeapItem {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            // Reverse for min-heap
            other.key.cmp(&self.key)
        }
    }

    impl PartialOrd for HeapItem {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    let mut chunk_readers: Vec<ChunkReader> = Vec::new();
    let mut heap: BinaryHeap<HeapItem> = BinaryHeap::new();

    for (idx, chunk_path) in chunk_files.iter().enumerate() {
        let file = File::open(chunk_path)?;
        let mut reader = ChunkReader {
            reader: BufReader::with_capacity(1024 * 1024, file),
            current: None,
            chunk_idx: idx,
        };
        reader.read_next()?;
        if let Some(ref record) = reader.current {
            heap.push(HeapItem {
                key: (record.prevout_txid, record.prevout_idx),
                chunk_idx: idx,
            });
        }
        chunk_readers.push(reader);
    }

    // Merge to output - delete chunks as they're exhausted to save space
    let mut writer = BufWriter::with_capacity(64 * 1024 * 1024, File::create(output_file)?);
    let mut merged = 0u64;
    let mut last_report = Instant::now();
    let mut chunk_exhausted = vec![false; chunk_files.len()];

    while let Some(item) = heap.pop() {
        let reader = &mut chunk_readers[item.chunk_idx];
        if let Some(ref record) = reader.current {
            writer.write_all(&record.to_bytes())?;
            merged += 1;
        }

        reader.read_next()?;
        if let Some(ref record) = reader.current {
            heap.push(HeapItem {
                key: (record.prevout_txid, record.prevout_idx),
                chunk_idx: item.chunk_idx,
            });
        } else if !chunk_exhausted[item.chunk_idx] {
            // Chunk is exhausted - delete it to free disk space
            chunk_exhausted[item.chunk_idx] = true;
            let _ = std::fs::remove_file(&chunk_files[item.chunk_idx]);
            println!("    Deleted exhausted chunk {}", item.chunk_idx);
        }

        if merged % 10_000_000 == 0 || last_report.elapsed().as_secs() >= 10 {
            println!(
                "    Merged: {} / {} ({:.1}%)",
                merged,
                num_records,
                merged as f64 / num_records as f64 * 100.0
            );
            last_report = Instant::now();
        }
    }

    writer.flush()?;

    // Cleanup any remaining temp files
    for chunk_path in &chunk_files {
        let _ = std::fs::remove_file(chunk_path);
    }
    let _ = std::fs::remove_dir(&temp_dir);

    let elapsed = start_time.elapsed();
    let file_size = std::fs::metadata(output_file)?.len();

    println!("  ✅ Step 2 Complete!");
    println!(
        "  Output: {} records ({:.2} GB)",
        merged,
        file_size as f64 / 1_073_741_824.0
    );
    println!("  Time: {:.1}m", elapsed.as_secs_f64() / 60.0);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_ref_serialization() {
        let record = InputRef {
            prevout_txid: [0x42; 32],
            prevout_idx: 0x12345678,
            block_height: 100000,
            tx_idx: 50,
            input_idx: 2,
        };

        let bytes = record.to_bytes();
        let decoded = InputRef::from_bytes(&bytes);

        assert_eq!(record.prevout_txid, decoded.prevout_txid);
        assert_eq!(record.prevout_idx, decoded.prevout_idx);
        assert_eq!(record.block_height, decoded.block_height);
        assert_eq!(record.tx_idx, decoded.tx_idx);
        assert_eq!(record.input_idx, decoded.input_idx);
    }
}
