//! Step 3: Extract outputs from all blocks
//!
//! For each transaction output, record:
//! - txid (32 bytes): Transaction ID
//! - output_idx (4 bytes): Output index within transaction
//! - block_height (4 bytes): Block height (needed for coinbase maturity check)
//! - is_coinbase (1 byte): Whether this is a coinbase output
//! - value (8 bytes): Output value in satoshis
//! - script_len (4 bytes): Length of scriptPubKey (u32; supports large OP_RETURN / data carrier outputs)
//! - script_pubkey (variable): The scriptPubKey
//!
//! Variable size due to scriptPubKey, average ~50 bytes per record.
//! ~2.5B outputs, but we can filter to only spent ones during merge.

use anyhow::{Context, Result};
use rayon::prelude::*;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::time::Instant;

use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
use blvm_protocol::transaction::is_coinbase;
use blvm_protocol::types::Hash;

use crate::chunked_cache::ChunkedBlockIterator;

/// Fixed header size before variable scriptPubKey.
/// txid(32) + output_idx(4) + block_height(4) + is_coinbase(1) + value(8) + script_len(4)
pub const OUTPUT_REF_HEADER_LEN: usize = 53;

/// Maximum scriptPubKey length (consensus max transaction serialized size).
pub const MAX_SCRIPT_PUBKEY_LEN: usize = 1_000_000;

/// Output record (variable size)
/// Header: 53 bytes fixed, plus variable scriptPubKey
#[derive(Debug, Clone)]
pub struct OutputRef {
    pub txid: Hash,
    pub output_idx: u32,
    pub block_height: u32,
    pub is_coinbase: bool,
    pub value: i64,
    pub script_pubkey: Vec<u8>,
}

impl OutputRef {
    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        debug_assert!(
            self.script_pubkey.len() <= MAX_SCRIPT_PUBKEY_LEN,
            "scriptPubKey length {} exceeds MAX_SCRIPT_PUBKEY_LEN",
            self.script_pubkey.len()
        );
        let mut buf = Vec::with_capacity(OUTPUT_REF_HEADER_LEN + self.script_pubkey.len());
        buf.extend_from_slice(&self.txid);
        buf.extend_from_slice(&self.output_idx.to_le_bytes());
        buf.extend_from_slice(&self.block_height.to_le_bytes());
        buf.push(if self.is_coinbase { 1 } else { 0 });
        buf.extend_from_slice(&self.value.to_le_bytes());
        let script_len: u32 = self
            .script_pubkey
            .len()
            .try_into()
            .expect("scriptPubKey length exceeds u32::MAX");
        buf.extend_from_slice(&script_len.to_le_bytes());
        buf.extend_from_slice(&self.script_pubkey);
        buf
    }

    /// Read from bytes, returns (record, bytes_consumed)
    pub fn from_bytes(buf: &[u8]) -> Option<(Self, usize)> {
        if buf.len() < OUTPUT_REF_HEADER_LEN {
            return None;
        }

        let mut txid = [0u8; 32];
        txid.copy_from_slice(&buf[0..32]);
        let output_idx = u32::from_le_bytes([buf[32], buf[33], buf[34], buf[35]]);
        let block_height = u32::from_le_bytes([buf[36], buf[37], buf[38], buf[39]]);
        let is_coinbase = buf[40] != 0;
        let value = i64::from_le_bytes([
            buf[41], buf[42], buf[43], buf[44], buf[45], buf[46], buf[47], buf[48],
        ]);
        let script_len = u32::from_le_bytes([buf[49], buf[50], buf[51], buf[52]]) as usize;

        if buf.len() < OUTPUT_REF_HEADER_LEN + script_len {
            return None;
        }

        let script_pubkey = buf[OUTPUT_REF_HEADER_LEN..OUTPUT_REF_HEADER_LEN + script_len].to_vec();

        Some((
            Self {
                txid,
                output_idx,
                block_height,
                is_coinbase,
                value,
                script_pubkey,
            },
            OUTPUT_REF_HEADER_LEN + script_len,
        ))
    }
}

/// Calculate txid from transaction
/// CRITICAL: Must use the SAME txid calculation as blvm-consensus to ensure merge-join works
/// Step 1 reads prevout.hash (which is the txid), and step 3 calculates txid - they MUST match
fn calculate_txid(tx: &blvm_protocol::types::Transaction) -> Hash {
    // Use the same calculate_tx_id function from blvm-consensus to ensure consistency
    use blvm_protocol::block::calculate_tx_id;
    calculate_tx_id(tx)
}

/// Stream the outputs file and verify record count (fast by default).
/// Set `STEP3_VERIFY_STRICT=1` for full round-trip decode on every record.
fn verify_output_file(output_file: &Path, expected_count: u64, file_size: u64) -> Result<()> {
    let strict = std::env::var("STEP3_VERIFY_STRICT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    println!(
        "  Verifying write integrity ({} count, {:.2} GB)...",
        if strict {
            "strict round-trip"
        } else {
            "fast streaming"
        },
        file_size as f64 / 1_073_741_824.0
    );

    let vf = File::open(output_file).with_context(|| {
        format!(
            "Failed to open output file for verification: {}",
            output_file.display()
        )
    })?;
    let mut vreader = BufReader::with_capacity(64 * 1024 * 1024, vf);
    let mut vbuf = vec![0u8; 512 * 1024];
    let mut vleftover: Vec<u8> = Vec::with_capacity(1024 * 1024);
    let mut cursor = 0usize;
    let mut verified_count = 0u64;
    let mut file_offset = 0u64;
    let mut veof = false;
    let verify_start = Instant::now();
    let mut last_report = Instant::now();

    loop {
        compact_output_leftover(&mut vleftover, &mut cursor);

        if let Some(record_len) = output_record_len_at(&vleftover, cursor) {
            if strict {
                let on_disk = &vleftover[cursor..cursor + record_len];
                let (rec, consumed) = OutputRef::from_bytes(on_disk).ok_or_else(|| {
                    anyhow::anyhow!("strict verify: parse failed at offset {}", file_offset)
                })?;
                let roundtrip = rec.to_bytes();
                if roundtrip != on_disk || consumed != record_len {
                    anyhow::bail!(
                        "Step 3 strict integrity: round-trip mismatch at record {} (offset {})",
                        verified_count,
                        file_offset,
                    );
                }
            }

            cursor += record_len;
            file_offset += record_len as u64;
            verified_count += 1;

            if last_report.elapsed().as_secs() >= 15 {
                let pct = file_offset as f64 / file_size as f64 * 100.0;
                let elapsed = verify_start.elapsed().as_secs_f64();
                let eta_min = if pct > 0.0 {
                    (elapsed / pct * (100.0 - pct)) / 60.0
                } else {
                    0.0
                };
                let rate_mb_s = file_offset as f64 / elapsed / 1_048_576.0;
                println!(
                    "  Verify: {:.1}% ({:.1}/{:.1} GB) - {} / {} records - {:.0} MB/s - ETA {:.0}m",
                    pct,
                    file_offset as f64 / 1_073_741_824.0,
                    file_size as f64 / 1_073_741_824.0,
                    verified_count,
                    expected_count,
                    rate_mb_s,
                    eta_min,
                );
                last_report = Instant::now();
            }
            continue;
        }

        if veof {
            break;
        }

        let n = vreader.read(&mut vbuf)?;
        if n == 0 {
            veof = true;
        } else {
            vleftover.extend_from_slice(&vbuf[..n]);
        }
    }

    let trailing = vleftover.len().saturating_sub(cursor);

    if verified_count != expected_count {
        anyhow::bail!(
            "Step 3 write integrity check FAILED: wrote {} records but file contains {} \
             (delta {}); {} leftover bytes at EOF (offset {}); preview [{}]. \
             Delete {} and re-run step 3.",
            expected_count,
            verified_count,
            expected_count as i64 - verified_count as i64,
            trailing,
            file_offset,
            if trailing > 0 {
                vleftover[cursor..cursor + trailing.min(32)]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            } else {
                String::new()
            },
            output_file.display(),
        );
    }

    if trailing > 0 {
        let preview: String = vleftover[cursor..cursor + trailing.min(32)]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        anyhow::bail!(
            "Step 3 write integrity check FAILED: {} trailing bytes after {} records at offset {}",
            trailing,
            verified_count,
            file_offset,
        );
    }

    let verify_elapsed = verify_start.elapsed();
    println!(
        "  ✅ Write integrity verified: {} records confirmed in {:.1}m{}",
        verified_count,
        verify_elapsed.as_secs_f64() / 60.0,
        if strict { " (strict)" } else { "" },
    );
    Ok(())
}

/// Extract all outputs from blocks and write to file
pub fn extract_outputs(
    chunks_dir: &Path,
    output_file: &Path,
    start_height: u64,
    end_height: u64,
    progress_interval: u64,
) -> Result<u64> {
    println!("\n{}", "═".repeat(60));
    println!("STEP 3: Extract Transaction Outputs");
    println!("{}", "═".repeat(60));
    println!("  Chunks dir: {}", chunks_dir.display());
    println!("  Blocks: {start_height} to {end_height}");
    println!("  Output: {}", output_file.display());

    let start_time = Instant::now();

    // Check if output file exists and find last processed block height
    let mut actual_start_height = start_height;
    let mut file_mode = std::fs::OpenOptions::new();
    let file_exists = output_file.exists();

    if file_exists {
        println!("  📍 Output file exists, checking last processed block...");
        // OPTIMIZATION: Use the same approach as check_last_outputs - read last chunk and parse
        use std::io::Seek;
        let existing_file = File::open(output_file).with_context(|| {
            format!(
                "Failed to open existing output file: {}",
                output_file.display()
            )
        })?;
        let file_size = existing_file.metadata()?.len();

        // Read last 100MB (should contain many records with highest block heights)
        // Larger size ensures we get valid records even if we start mid-record
        let chunk_size = (100 * 1024 * 1024).min(file_size);
        let start_offset = file_size - chunk_size;

        let mut reader = BufReader::new(existing_file);
        reader.seek(std::io::SeekFrom::Start(start_offset))?;
        let mut buf = vec![0u8; chunk_size as usize];
        reader.read_exact(&mut buf)?;

        // Parse all records from this chunk (same logic as check_last_outputs)
        let mut records: Vec<OutputRef> = Vec::new();
        let mut pos = 0;

        while pos < buf.len() {
            if let Some((record, consumed)) = OutputRef::from_bytes(&buf[pos..]) {
                // Validate: reasonable block height (< 1M blocks) and reasonable value
                if record.block_height < 1_000_000
                    && record.value >= 0
                    && record.value < 21_000_000_000_000_000
                {
                    records.push(record);
                }
                pos += consumed;
            } else {
                // Can't parse more - might be incomplete record at end
                break;
            }
        }

        // Find max block_height from parsed records
        let mut max_block_height = records
            .iter()
            .map(|r| r.block_height as u64)
            .max()
            .unwrap_or(0);

        // If parsing failed or gave garbage, use known value from check_last_outputs tool
        // The tool confirmed last output is from block 762154
        if max_block_height == 0 || max_block_height >= 1_000_000 {
            println!(
                "  ⚠️  Parsing gave invalid result ({max_block_height}), using known fallback: 762154"
            );
            max_block_height = 762154; // Known from check_last_outputs tool
        }

        println!(
            "  ✅ Scanned last {}MB, found {} records, max block height: {}",
            chunk_size / (1024 * 1024),
            records.len(),
            max_block_height
        );

        if max_block_height > 0 && max_block_height < 1_000_000 {
            actual_start_height = max_block_height + 1;
            println!(
                "  ✅ Found {} records in last {}MB, last processed block: {}",
                records.len(),
                chunk_size / (1024 * 1024),
                max_block_height
            );
            println!(
                "  📍 Resuming from block {actual_start_height} (will append to existing file)"
            );

            if actual_start_height >= end_height {
                println!("  ✅ All blocks already processed (up to {max_block_height})");
                return Ok(records.len() as u64);
            }
        } else {
            println!("  ⚠️  Couldn't determine last block, using fallback: 762154");
            actual_start_height = 762155; // Resume from known last block + 1
        }
    }

    // Create or append to output file
    let file = file_mode
        .create(true)
        .append(file_exists && actual_start_height > start_height)
        .write(true)
        .open(output_file)
        .with_context(|| format!("Failed to open output file: {}", output_file.display()))?;
    let mut writer = BufWriter::with_capacity(64 * 1024 * 1024, file); // 64MB buffer

    // Create block iterator starting from actual_start_height
    // CRITICAL FIX: Calculate max_blocks from end_height to ensure we process all requested blocks
    // Note: Iterator will still stop at metadata.total_blocks if chunks don't have all blocks,
    // but this ensures we try to process up to end_height
    let max_blocks = (end_height - actual_start_height) as usize;
    let mut block_iter =
        ChunkedBlockIterator::new(chunks_dir, Some(actual_start_height), Some(max_blocks))?
            .ok_or_else(|| {
                anyhow::anyhow!("Failed to create block iterator - chunks.meta not found?")
            })?;

    // Log the actual end_height the iterator will use (may be limited by metadata.total_blocks)
    eprintln!(
        "  📍 Block iterator configured: start={actual_start_height}, requested_end={end_height}, max_blocks={max_blocks}"
    );

    let mut total_outputs = 0u64;
    let mut bytes_written = 0u64;
    let mut height = actual_start_height;
    let mut last_report = Instant::now();

    while height < end_height {
        // Get next block
        let block_data = match block_iter.next_block()? {
            Some(data) => data,
            None => {
                if height < end_height {
                    eprintln!(
                        "  ⚠️  WARNING: Block iterator ended at height {height} but end_height is {end_height}"
                    );
                    eprintln!(
                        "  Missing blocks: {} to {} ({} blocks)",
                        height,
                        end_height - 1,
                        end_height - height
                    );
                    eprintln!(
                        "  This will cause missing prevouts for transactions in blocks {} to {}",
                        height,
                        end_height - 1
                    );
                    eprintln!("  Possible causes:");
                    eprintln!("    1. Chunks don't contain all blocks up to {end_height}");
                    eprintln!(
                        "    2. Chunk metadata (chunks.meta) reports fewer blocks than available"
                    );
                    eprintln!("    3. Block index is incomplete");
                    eprintln!("  Solution: Ensure chunks contain all blocks up to {end_height} or update chunks.meta");
                }
                // Stop extraction - we've processed all available blocks
                break;
            }
        };

        // Deserialize block
        let (block, _witnesses) = deserialize_block_with_witnesses(&block_data)
            .with_context(|| format!("Failed to deserialize block {height}"))?;

        // Process each transaction
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            let txid = calculate_txid(tx);
            let tx_is_coinbase = is_coinbase(tx);

            // Record each output
            for (output_idx, output) in tx.outputs.iter().enumerate() {
                if output.script_pubkey.len() > MAX_SCRIPT_PUBKEY_LEN {
                    anyhow::bail!(
                        "Step 3: scriptPubKey length {} exceeds maximum ({}) at block {}, \
                         tx index {}, output {} (txid {}).",
                        output.script_pubkey.len(),
                        MAX_SCRIPT_PUBKEY_LEN,
                        height,
                        tx_idx,
                        output_idx,
                        hex::encode(txid),
                    );
                }

                let record = OutputRef {
                    txid,
                    output_idx: output_idx as u32,
                    block_height: height as u32,
                    is_coinbase: tx_is_coinbase,
                    value: output.value,
                    script_pubkey: output.script_pubkey.clone(),
                };

                let bytes = record.to_bytes();
                writer.write_all(&bytes)?;
                bytes_written += bytes.len() as u64;
                total_outputs += 1;
            }

            // Silence unused variable warning
            let _ = tx_idx;
        }

        // Progress report
        let processed = height - start_height + 1;
        if processed % progress_interval == 0 || last_report.elapsed().as_secs() >= 10 {
            let elapsed = start_time.elapsed().as_secs_f64();
            let rate = processed as f64 / elapsed;
            let remaining = (end_height - height) as f64 / rate;
            println!(
                "  Block {}/{} ({:.1}%) - {} outputs - {:.0} blk/s - ETA: {:.0}m",
                height,
                end_height,
                (height - start_height) as f64 / (end_height - start_height) as f64 * 100.0,
                total_outputs,
                rate,
                remaining / 60.0
            );
            last_report = Instant::now();
        }

        height += 1;
    }

    writer.flush()?;
    // Drop the BufWriter (and the underlying File) so the OS flushes the inode.
    drop(writer);

    let elapsed = start_time.elapsed();
    let file_size = std::fs::metadata(output_file)?.len();

    if bytes_written != file_size {
        anyhow::bail!(
            "Step 3 size mismatch: tracked {} bytes written but file size is {} \
             (delta {} bytes). File may be corrupt; delete {} and re-run step 3.",
            bytes_written,
            file_size,
            file_size as i64 - bytes_written as i64,
            output_file.display(),
        );
    }

    // Post-write verification: stream back through the file and count records.
    verify_output_file(output_file, total_outputs, file_size)?;

    println!("{}", "─".repeat(60));
    println!("  ✅ Step 3 Complete!");
    println!("  Total outputs: {total_outputs}");
    println!("  Blocks processed: {}", height - start_height);
    println!("  File size: {:.2} GB", file_size as f64 / 1_073_741_824.0);
    println!("  Time: {:.1}m", elapsed.as_secs_f64() / 60.0);
    println!(
        "  Rate: {:.0} blocks/sec",
        (height - start_height) as f64 / elapsed.as_secs_f64()
    );

    Ok(total_outputs)
}

/// Sort key comparison on raw on-disk OutputRef bytes (txid + output_idx only).
pub(crate) fn output_record_key_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    a[0..32].cmp(&b[0..32]).then_with(|| {
        let ai = u32::from_le_bytes([a[32], a[33], a[34], a[35]]);
        let bi = u32::from_le_bytes([b[32], b[33], b[34], b[35]]);
        ai.cmp(&bi)
    })
}

fn output_record_key(rec: &[u8]) -> ([u8; 32], u32) {
    let mut txid = [0u8; 32];
    txid.copy_from_slice(&rec[0..32]);
    let idx = u32::from_le_bytes([rec[32], rec[33], rec[34], rec[35]]);
    (txid, idx)
}

/// Total byte length of one OutputRef record at `buf[offset..]`, if complete.
pub(crate) fn output_record_len_at(buf: &[u8], offset: usize) -> Option<usize> {
    if offset + OUTPUT_REF_HEADER_LEN > buf.len() {
        return None;
    }
    let script_len = u32::from_le_bytes([
        buf[offset + 49],
        buf[offset + 50],
        buf[offset + 51],
        buf[offset + 52],
    ]) as usize;
    if script_len > MAX_SCRIPT_PUBKEY_LEN {
        return None;
    }
    let total = OUTPUT_REF_HEADER_LEN + script_len;
    if offset + total > buf.len() {
        return None;
    }
    Some(total)
}

/// Take one raw record from `buf` at `cursor`, advancing the cursor.
pub(crate) fn take_raw_output_record(buf: &[u8], cursor: &mut usize) -> Option<Vec<u8>> {
    let len = output_record_len_at(buf, *cursor)?;
    let record = buf[*cursor..*cursor + len].to_vec();
    *cursor += len;
    Some(record)
}

/// Drop consumed prefix without shifting on every record.
pub(crate) fn compact_output_leftover(leftover: &mut Vec<u8>, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    if *cursor >= leftover.len() {
        leftover.clear();
        *cursor = 0;
    } else if *cursor > leftover.len() / 2 || *cursor > 256 * 1024 {
        leftover.copy_within(*cursor.., 0);
        leftover.truncate(leftover.len() - *cursor);
        *cursor = 0;
    }
}

fn sort_env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
        .max(1)
}

/// Read up to `chunk_records` raw OutputRef records from a streaming buffer.
fn read_raw_output_chunk(
    reader: &mut BufReader<File>,
    read_buf: &mut [u8],
    leftover: &mut Vec<u8>,
    cursor: &mut usize,
    eof: &mut bool,
    chunk_records: usize,
) -> Result<Vec<Vec<u8>>> {
    let mut records = Vec::with_capacity(chunk_records.min(1_000_000));

    while records.len() < chunk_records {
        compact_output_leftover(leftover, cursor);

        if let Some(record) = take_raw_output_record(leftover, cursor) {
            records.push(record);
            continue;
        }

        if *eof {
            break;
        }

        if leftover.len().saturating_sub(*cursor) >= 4 * 1024 * 1024 {
            anyhow::bail!(
                "sort_outputs Phase 1: parse stall with {} unconsumed bytes \
                 (file corrupted or unexpected record format)",
                leftover.len().saturating_sub(*cursor)
            );
        }

        let n = reader.read(read_buf)?;
        if n == 0 {
            *eof = true;
        } else {
            leftover.extend_from_slice(&read_buf[..n]);
        }
    }

    Ok(records)
}

fn sort_and_write_raw_chunk(
    mut records: Vec<Vec<u8>>,
    temp_dir: &Path,
    chunk_idx: usize,
) -> Result<(std::path::PathBuf, u64)> {
    records.sort_unstable_by(|a, b| output_record_key_cmp(a, b));

    let chunk_path = temp_dir.join(format!("chunk_{chunk_idx}.bin"));
    let mut chunk_writer = BufWriter::with_capacity(64 * 1024 * 1024, File::create(&chunk_path)?);
    for record in &records {
        chunk_writer.write_all(record)?;
    }
    chunk_writer.flush()?;

    Ok((chunk_path, records.len() as u64))
}

/// Sort outputs file by (txid, output_idx) using binary external merge sort
///
/// Phase 1 reads raw on-disk records (no decode/re-encode), sorts by key bytes,
/// and writes temp chunks. A background reader thread overlaps disk read with sort/write.
/// Tune via `SORT_PARALLEL_BATCH` (default 4) and `SORT_CHUNK_RECORDS` (default 40M).
pub fn sort_outputs(input_file: &Path, output_file: &Path) -> Result<()> {
    use std::collections::BinaryHeap;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    println!("\n{}", "═".repeat(60));
    println!("STEP 3b: Sort Outputs by TxID");
    println!("{}", "═".repeat(60));
    println!("  Input: {}", input_file.display());
    println!("  Output: {}", output_file.display());

    let start_time = Instant::now();

    let input_meta = std::fs::metadata(input_file)?;
    let input_size = input_meta.len();
    let input_mtime = input_meta.modified()?;
    println!(
        "  Input size: {:.2} GB",
        input_size as f64 / 1_073_741_824.0
    );

    // SAFETY CHECK: If output exists, verify it was created from the same input file
    if output_file.exists() {
        let output_meta = std::fs::metadata(output_file)?;
        let output_mtime = output_meta.modified()?;

        if input_mtime > output_mtime {
            eprintln!("\n  ⚠️  WARNING: Output file exists but input file is NEWER!");
            eprintln!("     Input modified:  {input_mtime:?}");
            eprintln!("     Output modified: {output_mtime:?}");
            eprintln!("     This means the input file was updated AFTER the output was created.");
            eprintln!("     The output file is likely INCOMPLETE and should be regenerated.");
            eprintln!("\n  Options:");
            eprintln!("     1. Delete {} and re-run step3b", output_file.display());
            eprintln!("     2. Continue anyway (NOT RECOMMENDED - will cause merge-join failures)");
            eprintln!(
                "\n  Aborting to prevent incomplete data. Delete the output file to proceed."
            );
            anyhow::bail!(
                "Output file is outdated - input file was modified after output was created. Delete {} to regenerate.",
                output_file.display()
            );
        }

        let output_size = output_meta.len();
        let size_diff_pct =
            ((input_size as f64 - output_size as f64) / input_size as f64 * 100.0).abs();
        if size_diff_pct > 5.0 {
            eprintln!("\n  ⚠️  WARNING: Input and output file sizes differ significantly!");
            eprintln!(
                "     Input size:  {:.2} GB",
                input_size as f64 / 1_073_741_824.0
            );
            eprintln!(
                "     Output size: {:.2} GB",
                output_size as f64 / 1_073_741_824.0
            );
            eprintln!("     Difference: {size_diff_pct:.1}%");
            eprintln!("     This may indicate incomplete data.");
        }
    }

    let chunk_records = sort_env_usize("SORT_CHUNK_RECORDS", 40_000_000);
    let parallel_batch = sort_env_usize("SORT_PARALLEL_BATCH", 4);
    println!("  Config: SORT_CHUNK_RECORDS={chunk_records}, SORT_PARALLEL_BATCH={parallel_batch}");

    let temp_dir = input_file
        .parent()
        .unwrap_or(Path::new("."))
        .join("sort_tmp");
    std::fs::create_dir_all(&temp_dir)?;
    let temp_dir = Arc::new(temp_dir);

    // ── Phase 1: pipelined read + parallel raw-byte sort ─────────────────────
    println!("  Phase 1: Creating sorted chunks (pipelined read, raw-byte sort)...");

    let (chunk_tx, chunk_rx) = mpsc::sync_channel::<Vec<Vec<u8>>>(parallel_batch + 1);
    let input_path = input_file.to_path_buf();
    let chunk_records_reader = chunk_records;

    let reader_handle = std::thread::spawn(move || -> Result<()> {
        let file = File::open(&input_path)?;
        let mut reader = BufReader::with_capacity(64 * 1024 * 1024, file);
        let mut read_buf = vec![0u8; 512 * 1024];
        let mut leftover: Vec<u8> = Vec::with_capacity(1024 * 1024);
        let mut cursor = 0usize;
        let mut eof = false;

        loop {
            let records = read_raw_output_chunk(
                &mut reader,
                &mut read_buf,
                &mut leftover,
                &mut cursor,
                &mut eof,
                chunk_records_reader,
            )?;
            if records.is_empty() {
                break;
            }
            chunk_tx
                .send(records)
                .map_err(|e| anyhow::anyhow!("sort reader thread: channel closed: {e}"))?;
        }
        Ok(())
    });

    let chunk_files = Mutex::new(Vec::new());
    let total_records = Mutex::new(0u64);
    let next_chunk_idx = AtomicUsize::new(0);
    let bytes_read = AtomicUsize::new(0);
    let mut phase1_last_report = Instant::now();
    let mut pending: Vec<Vec<Vec<u8>>> = Vec::with_capacity(parallel_batch);

    let flush_pending = |pending: &mut Vec<Vec<Vec<u8>>>,
                         temp_dir: &Arc<std::path::PathBuf>,
                         chunk_files: &Mutex<Vec<std::path::PathBuf>>,
                         total_records: &Mutex<u64>,
                         next_chunk_idx: &AtomicUsize|
     -> Result<()> {
        if pending.is_empty() {
            return Ok(());
        }
        let batch: Vec<Vec<Vec<u8>>> = std::mem::take(pending);
        let base_idx = next_chunk_idx.fetch_add(batch.len(), AtomicOrdering::Relaxed);

        let results: Result<Vec<_>> = batch
            .into_par_iter()
            .enumerate()
            .map(|(i, records)| sort_and_write_raw_chunk(records, temp_dir, base_idx + i))
            .collect();

        let results = results?;
        let mut files = chunk_files.lock().unwrap();
        let mut total = total_records.lock().unwrap();
        for (i, (path, count)) in results.iter().enumerate() {
            if (base_idx + i) % 10 == 0 {
                println!("    Chunk {}: {} records", base_idx + i, count);
            }
            *total += count;
            files.push(path.clone());
        }
        Ok(())
    };

    while let Ok(records) = chunk_rx.recv() {
        bytes_read.fetch_add(
            records.iter().map(|r| r.len()).sum::<usize>(),
            AtomicOrdering::Relaxed,
        );
        pending.push(records);

        if pending.len() >= parallel_batch {
            flush_pending(
                &mut pending,
                &temp_dir,
                &chunk_files,
                &total_records,
                &next_chunk_idx,
            )?;
        }

        if phase1_last_report.elapsed().as_secs() >= 15 {
            let read_gb = bytes_read.load(AtomicOrdering::Relaxed) as f64 / 1_073_741_824.0;
            let pct = read_gb / (input_size as f64 / 1_073_741_824.0) * 100.0;
            let chunks_done = next_chunk_idx.load(AtomicOrdering::Relaxed);
            let records_done = *total_records.lock().unwrap();
            println!(
                "  Phase 1: {:.1}% read ({:.1}/{:.1} GB) - {} chunks - {} records - {:.1}m elapsed",
                pct,
                read_gb,
                input_size as f64 / 1_073_741_824.0,
                chunks_done,
                records_done,
                start_time.elapsed().as_secs_f64() / 60.0
            );
            phase1_last_report = Instant::now();
        }
    }

    reader_handle
        .join()
        .map_err(|_| anyhow::anyhow!("sort reader thread panicked"))??;

    flush_pending(
        &mut pending,
        &temp_dir,
        &chunk_files,
        &total_records,
        &next_chunk_idx,
    )?;

    let chunk_files = chunk_files.into_inner().unwrap();
    let total_records = total_records.into_inner().unwrap();
    println!(
        "  Phase 1 done: {} chunks, {} records, {:.1}m",
        chunk_files.len(),
        total_records,
        start_time.elapsed().as_secs_f64() / 60.0
    );

    // ── Phase 2: k-way merge on raw records ───────────────────────────────────
    println!("  Phase 2: Merging {} chunks...", chunk_files.len());

    struct RawChunkReader {
        reader: BufReader<File>,
        current: Option<Vec<u8>>,
        leftover: Vec<u8>,
        cursor: usize,
        chunk_idx: usize,
    }

    impl RawChunkReader {
        fn read_next(&mut self) -> Result<()> {
            let mut buf = vec![0u8; 512 * 1024];
            loop {
                compact_output_leftover(&mut self.leftover, &mut self.cursor);
                if let Some(record) = take_raw_output_record(&self.leftover, &mut self.cursor) {
                    self.current = Some(record);
                    return Ok(());
                }
                match self.reader.read(&mut buf)? {
                    0 => {
                        self.current = None;
                        return Ok(());
                    }
                    n => self.leftover.extend_from_slice(&buf[..n]),
                }
            }
        }
    }

    #[derive(Eq, PartialEq)]
    struct HeapItem {
        key: ([u8; 32], u32),
        chunk_idx: usize,
    }

    impl Ord for HeapItem {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            other.key.cmp(&self.key)
        }
    }

    impl PartialOrd for HeapItem {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    let mut chunk_readers: Vec<RawChunkReader> = Vec::new();
    let mut heap: BinaryHeap<HeapItem> = BinaryHeap::new();

    for (idx, chunk_path) in chunk_files.iter().enumerate() {
        let file = File::open(chunk_path)?;
        let mut reader = RawChunkReader {
            reader: BufReader::with_capacity(64 * 1024 * 1024, file),
            current: None,
            leftover: Vec::new(),
            cursor: 0,
            chunk_idx: idx,
        };
        reader.read_next()?;

        if let Some(ref record) = reader.current {
            heap.push(HeapItem {
                key: output_record_key(record),
                chunk_idx: idx,
            });
        }
        chunk_readers.push(reader);
    }

    let mut output_writer = BufWriter::with_capacity(64 * 1024 * 1024, File::create(output_file)?);
    let mut merged = 0u64;
    let progress_interval = (total_records / 100).max(1);

    while let Some(item) = heap.pop() {
        let reader = &mut chunk_readers[item.chunk_idx];
        if let Some(record) = reader.current.take() {
            output_writer.write_all(&record)?;
            merged += 1;

            if merged % progress_interval == 0 {
                println!(
                    "    Merged: {} / {} ({:.1}%)",
                    merged,
                    total_records,
                    merged as f64 / total_records as f64 * 100.0
                );
            }
        }

        reader.read_next()?;
        if let Some(ref record) = reader.current {
            heap.push(HeapItem {
                key: output_record_key(record),
                chunk_idx: item.chunk_idx,
            });
        } else {
            let _ = std::fs::remove_file(&chunk_files[item.chunk_idx]);
        }
    }

    output_writer.flush()?;
    let _ = std::fs::remove_dir_all(temp_dir.as_ref());

    let elapsed = start_time.elapsed();
    let output_size = std::fs::metadata(output_file)?.len();

    println!("{}", "─".repeat(60));
    println!("  ✅ Step 3b Complete!");
    println!(
        "  Output: {} records ({:.2} GB)",
        merged,
        output_size as f64 / 1_073_741_824.0
    );
    println!("  Time: {:.1}m", elapsed.as_secs_f64() / 60.0);

    Ok(())
}
