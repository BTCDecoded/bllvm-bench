//! Step 4: Merge-Join inputs with outputs to get prevout data
//!
//! Both files are sorted by (txid, index):
//! - Inputs sorted by (prevout_txid, prevout_idx)
//! - Outputs sorted by (txid, output_idx)
//!
//! Output: For each input, the prevout data needed for verification:
//! - block_height (spending block)
//! - tx_idx (spending transaction)
//! - input_idx (spending input)
//! - prevout_height (source block, for coinbase maturity)
//! - is_coinbase (source output)
//! - value (for SegWit sighash)
//! - script_pubkey (for verification)

use anyhow::Result;
use hex;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

use super::input_refs::InputRef;
use super::output_refs::{
    compact_output_leftover, take_raw_output_record, OutputRef, MAX_SCRIPT_PUBKEY_LEN,
};

/// Fixed header before variable scriptPubKey in joined prevout records.
/// spending(4+4+4) + prevout_height(4) + is_coinbase(1) + value(8) + script_len(4) = 29
pub const JOINED_PREVOUT_HEADER_LEN: usize = 29;

/// Joined prevout record (variable size)
#[derive(Debug, Clone)]
pub struct JoinedPrevout {
    /// Block height where this input is being spent
    pub spending_block: u32,
    /// Transaction index in the spending block
    pub spending_tx_idx: u32,
    /// Input index in the spending transaction
    pub spending_input_idx: u32,
    /// Block height where the prevout was created
    pub prevout_height: u32,
    /// Whether the prevout is from a coinbase transaction
    pub is_coinbase: bool,
    /// Value of the prevout (for SegWit sighash calculation)
    pub value: i64,
    /// The scriptPubKey to verify against
    pub script_pubkey: Vec<u8>,
}

impl JoinedPrevout {
    pub fn to_bytes(&self) -> Vec<u8> {
        debug_assert!(self.script_pubkey.len() <= MAX_SCRIPT_PUBKEY_LEN);
        let mut buf = Vec::with_capacity(JOINED_PREVOUT_HEADER_LEN + self.script_pubkey.len());
        buf.extend_from_slice(&self.spending_block.to_le_bytes());
        buf.extend_from_slice(&self.spending_tx_idx.to_le_bytes());
        buf.extend_from_slice(&self.spending_input_idx.to_le_bytes());
        buf.extend_from_slice(&self.prevout_height.to_le_bytes());
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

    pub fn from_bytes(buf: &[u8]) -> Option<(Self, usize)> {
        if buf.len() < JOINED_PREVOUT_HEADER_LEN {
            return None;
        }

        let spending_block = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let spending_tx_idx = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let spending_input_idx = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let prevout_height = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let is_coinbase = buf[16] != 0;
        let value = i64::from_le_bytes([
            buf[17], buf[18], buf[19], buf[20], buf[21], buf[22], buf[23], buf[24],
        ]);
        let script_len = u32::from_le_bytes([buf[25], buf[26], buf[27], buf[28]]) as usize;

        if script_len > MAX_SCRIPT_PUBKEY_LEN {
            return None;
        }
        if buf.len() < JOINED_PREVOUT_HEADER_LEN + script_len {
            return None;
        }

        let script_pubkey =
            buf[JOINED_PREVOUT_HEADER_LEN..JOINED_PREVOUT_HEADER_LEN + script_len].to_vec();

        // Reject misaligned reads (variable-length records in sorted merge files).
        if spending_block == 0
            || spending_block >= 1_000_000
            || prevout_height >= 1_000_000
            || value < 0
        {
            return None;
        }

        Some((
            Self {
                spending_block,
                spending_tx_idx,
                spending_input_idx,
                prevout_height,
                is_coinbase,
                value,
                script_pubkey,
            },
            JOINED_PREVOUT_HEADER_LEN + script_len,
        ))
    }
}

fn joined_record_key_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    u32::from_le_bytes([a[0], a[1], a[2], a[3]])
        .cmp(&u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .then_with(|| {
            u32::from_le_bytes([a[4], a[5], a[6], a[7]])
                .cmp(&u32::from_le_bytes([b[4], b[5], b[6], b[7]]))
        })
        .then_with(|| {
            u32::from_le_bytes([a[8], a[9], a[10], a[11]])
                .cmp(&u32::from_le_bytes([b[8], b[9], b[10], b[11]]))
        })
}

pub(crate) fn joined_record_len_at(buf: &[u8], offset: usize) -> Option<usize> {
    if offset + JOINED_PREVOUT_HEADER_LEN > buf.len() {
        return None;
    }
    let script_len = u32::from_le_bytes([
        buf[offset + 25],
        buf[offset + 26],
        buf[offset + 27],
        buf[offset + 28],
    ]) as usize;
    if script_len > MAX_SCRIPT_PUBKEY_LEN {
        return None;
    }
    let total = JOINED_PREVOUT_HEADER_LEN + script_len;
    if offset + total > buf.len() {
        return None;
    }
    Some(total)
}

fn take_raw_joined_record(buf: &[u8], cursor: &mut usize) -> Option<Vec<u8>> {
    let len = joined_record_len_at(buf, *cursor)?;
    let record = buf[*cursor..*cursor + len].to_vec();
    *cursor += len;
    Some(record)
}

fn cmp_input_to_raw_output(input: &InputRef, output_raw: &[u8]) -> std::cmp::Ordering {
    let output_txid: [u8; 32] = output_raw[0..32]
        .try_into()
        .expect("output record missing txid");
    input.prevout_txid.cmp(&output_txid).then_with(|| {
        input.prevout_idx.cmp(&u32::from_le_bytes([
            output_raw[32],
            output_raw[33],
            output_raw[34],
            output_raw[35],
        ]))
    })
}

fn read_next_output_raw(
    reader: &mut BufReader<File>,
    read_buf: &mut [u8],
    leftover: &mut Vec<u8>,
    cursor: &mut usize,
) -> Result<Option<Vec<u8>>> {
    loop {
        compact_output_leftover(leftover, cursor);
        if let Some(record) = take_raw_output_record(leftover, cursor) {
            return Ok(Some(record));
        }
        let n = reader.read(read_buf)?;
        if n == 0 {
            return Ok(None);
        }
        leftover.extend_from_slice(&read_buf[..n]);
    }
}

fn sort_env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
        .max(1)
}

fn read_raw_joined_chunk(
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
        if let Some(record) = take_raw_joined_record(leftover, cursor) {
            records.push(record);
            continue;
        }
        if *eof {
            break;
        }
        if leftover.len().saturating_sub(*cursor) >= 4 * 1024 * 1024 {
            anyhow::bail!(
                "sort_joined Phase 1: parse stall with {} unconsumed bytes",
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

fn sort_and_write_raw_joined_chunk(
    mut records: Vec<Vec<u8>>,
    temp_dir: &Path,
    chunk_idx: usize,
) -> Result<(std::path::PathBuf, u64)> {
    records.sort_unstable_by(|a, b| joined_record_key_cmp(a, b));
    let chunk_path = temp_dir.join(format!("chunk_{chunk_idx}.bin"));
    let mut chunk_writer = BufWriter::with_capacity(64 * 1024 * 1024, File::create(&chunk_path)?);
    for record in &records {
        chunk_writer.write_all(record)?;
    }
    chunk_writer.flush()?;
    Ok((chunk_path, records.len() as u64))
}

/// Merge-join sorted inputs with sorted outputs
///
/// Both files must be sorted by (txid, index).
/// Outputs all inputs that have matching outputs (spent outputs).
pub fn merge_join(
    inputs_file: &Path,
    outputs_file: &Path,
    joined_file: &Path,
) -> Result<(u64, u64)> {
    println!("\n{}", "═".repeat(60));
    println!("STEP 4: Merge-Join Inputs with Outputs");
    println!("{}", "═".repeat(60));
    println!("  Inputs: {}", inputs_file.display());
    println!("  Outputs: {}", outputs_file.display());
    println!("  Joined: {}", joined_file.display());

    let start_time = Instant::now();

    // Check if we can resume from existing joined file
    // Inputs are sorted by (prevout_txid, prevout_idx), so we need to find
    // the last matched input's prevout key to resume correctly
    let mut resume_from_prevout: Option<([u8; 32], u32)> = None; // (prevout_txid, prevout_idx)
    let mut existing_joined_count = 0u64;
    let file_mode = std::fs::OpenOptions::new();
    let file_exists = joined_file.exists();

    if file_exists {
        println!("  📍 Joined file exists, checking if we can resume...");
        let joined_meta = std::fs::metadata(joined_file)?;
        let joined_size = joined_meta.len();

        if joined_size > 0 {
            // Read last 10MB to find last matched record
            let read_size = std::cmp::min(10 * 1024 * 1024, joined_size);
            let mut joined_reader = BufReader::new(File::open(joined_file)?);
            joined_reader.seek(SeekFrom::End(-(read_size as i64)))?;
            let mut buf = vec![0u8; 256 * 1024];

            let read_next_joined = |reader: &mut BufReader<File>,
                                    leftover: &mut Vec<u8>,
                                    cursor: &mut usize,
                                    buf: &mut [u8]|
             -> Result<Option<JoinedPrevout>> {
                loop {
                    compact_output_leftover(leftover, cursor);
                    if let Some(len) = joined_record_len_at(leftover, *cursor) {
                        let slice = &leftover[*cursor..*cursor + len];
                        if let Some((record, consumed)) = JoinedPrevout::from_bytes(slice) {
                            *cursor += consumed;
                            return Ok(Some(record));
                        }
                    }
                    let n = reader.read(buf)?;
                    if n == 0 {
                        return Ok(None);
                    }
                    leftover.extend_from_slice(&buf[..n]);
                }
            };

            let mut leftover = Vec::new();
            let mut cursor = 0usize;

            // Find last record
            let mut last_record: Option<JoinedPrevout> = None;
            while let Some(record) =
                read_next_joined(&mut joined_reader, &mut leftover, &mut cursor, &mut buf)?
            {
                last_record = Some(record);
            }

            if let Some(record) = last_record {
                println!(
                    "  ✅ Found last matched input: block {}, tx {}, input {}",
                    record.spending_block, record.spending_tx_idx, record.spending_input_idx
                );

                // Now find this input in the inputs file to get its prevout_txid/prevout_idx
                // (inputs are sorted by prevout, not by spending location, so we need to scan)
                println!("  🔍 Finding input's prevout key in inputs file...");
                let mut inputs_scan = BufReader::new(File::open(inputs_file)?);
                let mut input_scan_buf = [0u8; InputRef::SIZE];
                let mut found_prevout: Option<([u8; 32], u32)> = None;
                let mut scanned = 0u64;

                while inputs_scan.read_exact(&mut input_scan_buf).is_ok() {
                    let input = InputRef::from_bytes(&input_scan_buf);
                    scanned += 1;

                    if input.block_height == record.spending_block
                        && input.tx_idx == record.spending_tx_idx
                        && input.input_idx == record.spending_input_idx
                    {
                        found_prevout = Some((input.prevout_txid, input.prevout_idx));
                        println!(
                            "  ✅ Found prevout key: txid={}, idx={}",
                            hex::encode(input.prevout_txid),
                            input.prevout_idx
                        );
                        break;
                    }

                    if scanned % 10_000_000 == 0 {
                        println!("  ⏳ Scanned {}M inputs...", scanned / 1_000_000);
                    }
                }

                if let Some(prevout_key) = found_prevout {
                    resume_from_prevout = Some(prevout_key);
                    println!(
                        "  📍 Will resume from next input after prevout ({}, {})",
                        hex::encode(prevout_key.0),
                        prevout_key.1
                    );
                } else {
                    println!("  ⚠️  Could not find input in inputs file - will re-run from start");
                }

                // Count existing records
                let mut count_reader = BufReader::new(File::open(joined_file)?);
                let mut count_buf = vec![0u8; 256 * 1024];
                let mut count_leftover = Vec::new();
                let mut count_cursor = 0usize;
                while let Some(_) = read_next_joined(
                    &mut count_reader,
                    &mut count_leftover,
                    &mut count_cursor,
                    &mut count_buf,
                )? {
                    existing_joined_count += 1;
                }
                println!("  📊 Existing joined records: {existing_joined_count}");
            }
        }
    }

    let mut inputs_reader = BufReader::with_capacity(32 * 1024 * 1024, File::open(inputs_file)?);
    let mut outputs_reader = BufReader::with_capacity(32 * 1024 * 1024, File::open(outputs_file)?);

    // Open file for append if resuming, create if new, truncate if file exists but we're not resuming
    let mut writer = if resume_from_prevout.is_some() {
        // Resuming - append to existing file
        BufWriter::with_capacity(
            32 * 1024 * 1024,
            std::fs::OpenOptions::new()
                .create(false)
                .append(true)
                .open(joined_file)?,
        )
    } else if file_exists {
        // File exists but we're not resuming - truncate and start fresh
        println!("  ⚠️  File exists but resume not possible - will overwrite");
        BufWriter::with_capacity(
            32 * 1024 * 1024,
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(joined_file)?,
        )
    } else {
        // New file - create
        BufWriter::with_capacity(
            32 * 1024 * 1024,
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(joined_file)?,
        )
    };

    let mut joined_count = existing_joined_count;
    let mut unmatched_inputs = 0u64;
    let mut last_output_txid: Option<[u8; 32]> = None;

    // Skip inputs until we reach the resume point (sorted by prevout_txid, prevout_idx)
    let mut input_buf = [0u8; InputRef::SIZE];
    let mut current_input: Option<InputRef> = None;
    let mut current_output_raw: Option<Vec<u8>> = None;

    let mut output_buf = vec![0u8; 512 * 1024];
    let mut output_leftover = Vec::new();
    let mut output_cursor = 0usize;
    let mut outputs_exhausted = false;
    if let Some((resume_txid, resume_idx)) = resume_from_prevout {
        println!("  ⏩ Skipping inputs until resume point...");
        let mut skipped = 0u64;
        loop {
            if inputs_reader.read_exact(&mut input_buf).is_err() {
                break;
            }
            let input = InputRef::from_bytes(&input_buf);

            // Compare: (prevout_txid, prevout_idx) - inputs are sorted by this
            let cmp = input
                .prevout_txid
                .cmp(&resume_txid)
                .then_with(|| input.prevout_idx.cmp(&resume_idx));

            match cmp {
                std::cmp::Ordering::Less => {
                    skipped += 1;
                    continue; // Skip this input
                }
                std::cmp::Ordering::Equal => {
                    // Found the resume point, skip this one and start from next
                    skipped += 1;
                    if inputs_reader.read_exact(&mut input_buf).is_ok() {
                        current_input = Some(InputRef::from_bytes(&input_buf));
                    }
                    println!("  ✅ Resumed from input after {skipped} skipped inputs");
                    break;
                }
                std::cmp::Ordering::Greater => {
                    // We've passed the resume point, use this input
                    current_input = Some(input);
                    println!("  ✅ Resumed from input ({skipped} skipped)");
                    break;
                }
            }
        }

        // Also need to position outputs reader at the matching output
        // Outputs are sorted by (txid, output_idx), so find the output matching resume_txid/resume_idx
        println!("  ⏩ Positioning outputs reader at resume point...");
        let mut output_pos_found = false;

        while let Some(output_raw) = read_next_output_raw(
            &mut outputs_reader,
            &mut output_buf,
            &mut output_leftover,
            &mut output_cursor,
        )? {
            let cmp = resume_txid
                .cmp(&output_raw[0..32].try_into().unwrap())
                .then_with(|| {
                    resume_idx.cmp(&u32::from_le_bytes([
                        output_raw[32],
                        output_raw[33],
                        output_raw[34],
                        output_raw[35],
                    ]))
                });
            match cmp {
                std::cmp::Ordering::Less => continue,
                std::cmp::Ordering::Equal | std::cmp::Ordering::Greater => {
                    let output_idx = u32::from_le_bytes([
                        output_raw[32],
                        output_raw[33],
                        output_raw[34],
                        output_raw[35],
                    ]);
                    current_output_raw = Some(output_raw);
                    output_pos_found = true;
                    println!(
                        "  ✅ Positioned outputs reader at txid={}, idx={}",
                        hex::encode(&current_output_raw.as_ref().unwrap()[0..32]),
                        output_idx
                    );
                    break;
                }
            }
        }

        if !output_pos_found {
            println!("  ⚠️  Could not find matching output - starting from beginning of outputs");
            outputs_reader = BufReader::with_capacity(32 * 1024 * 1024, File::open(outputs_file)?);
            output_leftover.clear();
            output_cursor = 0;
            if let Some(output_raw) = read_next_output_raw(
                &mut outputs_reader,
                &mut output_buf,
                &mut output_leftover,
                &mut output_cursor,
            )? {
                current_output_raw = Some(output_raw);
                println!("  ✅ Initialized outputs reader from beginning");
            } else {
                outputs_exhausted = true;
                println!("  ⚠️  No outputs available - outputs file may be empty");
            }
        }
    } else {
        if inputs_reader.read_exact(&mut input_buf).is_ok() {
            current_input = Some(InputRef::from_bytes(&input_buf));
        }
        if let Some(output_raw) = read_next_output_raw(
            &mut outputs_reader,
            &mut output_buf,
            &mut output_leftover,
            &mut output_cursor,
        )? {
            current_output_raw = Some(output_raw);
        } else {
            outputs_exhausted = true;
        }
    }

    let mut last_report = Instant::now();

    // Merge-join loop
    while let Some(ref input) = current_input {
        if outputs_exhausted {
            // No more outputs - remaining inputs are unmatched
            unmatched_inputs += 1;

            // Read next input
            if inputs_reader.read_exact(&mut input_buf).is_ok() {
                current_input = Some(InputRef::from_bytes(&input_buf));
            } else {
                break;
            }
            continue;
        }

        let output_raw = current_output_raw.as_ref().unwrap();
        let cmp = cmp_input_to_raw_output(input, output_raw);

        match cmp {
            std::cmp::Ordering::Equal => {
                let (output, _) = OutputRef::from_bytes(output_raw)
                    .ok_or_else(|| anyhow::anyhow!("merge-join: invalid output record at match"))?;
                let joined = JoinedPrevout {
                    spending_block: input.block_height,
                    spending_tx_idx: input.tx_idx,
                    spending_input_idx: input.input_idx,
                    prevout_height: output.block_height,
                    is_coinbase: output.is_coinbase,
                    value: output.value,
                    script_pubkey: output.script_pubkey,
                };
                writer.write_all(&joined.to_bytes())?;
                joined_count += 1;

                if inputs_reader.read_exact(&mut input_buf).is_ok() {
                    current_input = Some(InputRef::from_bytes(&input_buf));
                } else {
                    current_input = None;
                }
            }
            std::cmp::Ordering::Less => {
                unmatched_inputs += 1;
                if inputs_reader.read_exact(&mut input_buf).is_ok() {
                    current_input = Some(InputRef::from_bytes(&input_buf));
                } else {
                    current_input = None;
                }
            }
            std::cmp::Ordering::Greater => {
                if let Some(next_raw) = read_next_output_raw(
                    &mut outputs_reader,
                    &mut output_buf,
                    &mut output_leftover,
                    &mut output_cursor,
                )? {
                    last_output_txid = Some(next_raw[0..32].try_into().expect("txid length"));
                    current_output_raw = Some(next_raw);
                } else {
                    if let Some(ref last_txid) = last_output_txid {
                        eprintln!(
                            "  ⚠️  Outputs exhausted at input prevout_txid: {}",
                            hex::encode(input.prevout_txid)
                        );
                        eprintln!("  Last output txid: {}", hex::encode(*last_txid));
                        eprintln!(
                            "  Input prevout_txid > Last output txid: {}",
                            input.prevout_txid > *last_txid
                        );
                    }
                    outputs_exhausted = true;
                }
            }
        }

        // Progress report every 10 seconds
        if last_report.elapsed().as_secs() >= 10 {
            println!("  Joined: {joined_count}, Unmatched: {unmatched_inputs}");
            last_report = Instant::now();
        }
    }

    writer.flush()?;

    let elapsed = start_time.elapsed();
    let file_size = std::fs::metadata(joined_file)?.len();

    println!("{}", "─".repeat(60));
    println!("  ✅ Step 4 Complete!");
    println!("  Joined records: {joined_count}");
    println!("  Unmatched inputs: {unmatched_inputs} (should be 0 for valid chain)");
    println!("  File size: {:.2} GB", file_size as f64 / 1_073_741_824.0);
    println!("  Time: {:.1}m", elapsed.as_secs_f64() / 60.0);

    Ok((joined_count, unmatched_inputs))
}

fn joined_record_key(record: &[u8]) -> (u32, u32, u32) {
    (
        u32::from_le_bytes([record[0], record[1], record[2], record[3]]),
        u32::from_le_bytes([record[4], record[5], record[6], record[7]]),
        u32::from_le_bytes([record[8], record[9], record[10], record[11]]),
    )
}

/// Sort joined file by (spending_block, spending_tx_idx, spending_input_idx) using binary merge sort
/// This puts prevouts in the exact order we'll need them during verification.
pub fn sort_joined(input_file: &Path, output_file: &Path) -> Result<()> {
    use rayon::prelude::*;
    use std::collections::BinaryHeap;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    println!("\n{}", "═".repeat(60));
    println!("STEP 5: Sort Joined Data by Spending Location");
    println!("{}", "═".repeat(60));
    println!("  Input: {}", input_file.display());
    println!("  Output: {}", output_file.display());

    let start_time = Instant::now();

    let input_meta = std::fs::metadata(input_file)?;
    let input_size = input_meta.len();
    println!(
        "  Input size: {:.2} GB",
        input_size as f64 / 1_073_741_824.0
    );

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
            let records = read_raw_joined_chunk(
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
                .map_err(|e| anyhow::anyhow!("sort_joined reader thread: channel closed: {e}"))?;
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
            .map(|(i, records)| sort_and_write_raw_joined_chunk(records, temp_dir, base_idx + i))
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
        .map_err(|_| anyhow::anyhow!("sort_joined reader thread panicked"))??;

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

    struct RawJoinedChunkReader {
        reader: BufReader<File>,
        current: Option<Vec<u8>>,
        leftover: Vec<u8>,
        cursor: usize,
        chunk_idx: usize,
    }

    impl RawJoinedChunkReader {
        fn read_next(&mut self) -> Result<()> {
            let mut buf = vec![0u8; 512 * 1024];
            loop {
                compact_output_leftover(&mut self.leftover, &mut self.cursor);
                if let Some(record) = take_raw_joined_record(&self.leftover, &mut self.cursor) {
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
        key: (u32, u32, u32),
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

    let mut chunk_readers: Vec<RawJoinedChunkReader> = Vec::new();
    let mut heap: BinaryHeap<HeapItem> = BinaryHeap::new();

    for (idx, chunk_path) in chunk_files.iter().enumerate() {
        let file = File::open(chunk_path)?;
        let mut reader = RawJoinedChunkReader {
            reader: BufReader::with_capacity(64 * 1024 * 1024, file),
            current: None,
            leftover: Vec::new(),
            cursor: 0,
            chunk_idx: idx,
        };
        reader.read_next()?;

        if let Some(ref record) = reader.current {
            heap.push(HeapItem {
                key: joined_record_key(record),
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
                key: joined_record_key(record),
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
    println!("  ✅ Step 5 Complete!");
    println!(
        "  Output: {} records ({:.2} GB)",
        merged,
        output_size as f64 / 1_073_741_824.0
    );
    println!("  Time: {:.1}m", elapsed.as_secs_f64() / 60.0);

    Ok(())
}
