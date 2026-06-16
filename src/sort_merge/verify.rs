//! Step 6: Parallel script verification
//!
//! Streams blocks and prevouts in lockstep, verifying scripts using all CPU cores.
//! The prevout file is sorted by (block, tx, input), so we read it sequentially.

use anyhow::{Context, Result};
use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;

use blvm_protocol::activation::{ForkActivationTable, IsForkActive};
use blvm_protocol::bip113::get_median_time_past;
use blvm_protocol::block::calculate_base_script_flags_for_block_network;
use blvm_protocol::block::{tx_has_nonempty_input_witness, tx_requires_witness_script_flags};
use blvm_protocol::script::{verify_script_with_context_full, SigVersion};
use blvm_protocol::segwit::Witness;
use blvm_protocol::serialization::block::{
    deserialize_block_header, deserialize_block_with_witnesses,
};
use blvm_protocol::serialization::transaction::serialize_transaction;
use blvm_protocol::transaction::is_coinbase;
use blvm_protocol::types::ForkId;
use blvm_protocol::types::{BlockHeader, Network};
use blvm_protocol::witness::is_witness_empty;

use blvm_consensus::bip348::SchnorrSignatureCollector;

use super::merge_join::{JoinedPrevout, JOINED_PREVOUT_HEADER_LEN};
use crate::chunked_cache::ChunkedBlockIterator;
use hex;

/// Base script flags for `(height, network)` — matches `blvm-consensus` block connect / `script_cache`.
pub fn get_script_flags(height: u64, network: Network) -> u32 {
    calculate_base_script_flags_for_block_network(height, network)
}

/// Prevout reader that streams sorted prevout data
pub struct PrevoutReader {
    reader: BufReader<File>,
    buffer: Vec<u8>,
    leftover: Vec<u8>,
}

impl PrevoutReader {
    pub fn new(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("Failed to open prevout file: {}", path.display()))?;
        Ok(Self {
            reader: BufReader::with_capacity(64 * 1024 * 1024, file),
            buffer: vec![0u8; 256 * 1024],
            leftover: Vec::new(),
        })
    }

    /// Parse the first valid record at or after `data[offset..]`.
    fn parse_record_at(data: &[u8], offset: usize) -> Option<(JoinedPrevout, usize)> {
        let mut pos = offset;
        let scan_limit = (offset + 512).min(data.len());
        while pos + JOINED_PREVOUT_HEADER_LEN <= scan_limit {
            if let Some((record, consumed)) = JoinedPrevout::from_bytes(&data[pos..]) {
                if record.spending_block > 0 && record.spending_block < 1_000_000 {
                    return Some((record, pos + consumed));
                }
            }
            pos += 1;
        }
        None
    }

    /// Byte offset of the first plausible record within the first 512 bytes (seek landing only).
    fn find_first_record_start(data: &[u8]) -> Option<usize> {
        let scan_limit = data.len().min(512);
        for start in 0..scan_limit {
            if start + JOINED_PREVOUT_HEADER_LEN <= data.len() {
                if let Some((record, _)) = JoinedPrevout::from_bytes(&data[start..]) {
                    if record.spending_block > 0 && record.spending_block < 1_000_000 {
                        return Some(start);
                    }
                }
            }
        }
        None
    }

    /// One-time prefix alignment after binary seek (never used during sequential block reads).
    fn align_leftover_prefix(leftover: &mut Vec<u8>) -> Result<()> {
        if JoinedPrevout::from_bytes(leftover).is_some() {
            return Ok(());
        }
        let start = Self::find_first_record_start(leftover)
            .ok_or_else(|| anyhow::anyhow!("could not align joined_sorted.bin within 512 bytes"))?;
        if start > 0 {
            leftover.drain(..start);
        }
        Ok(())
    }

    /// Binary-search `joined_sorted.bin` (sorted by spending_block) to seek near `target_height`.
    fn binary_seek_to_block(reader: &mut BufReader<File>, target_height: u32) -> Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};

        let file_size = reader.get_ref().metadata()?.len();
        if file_size == 0 {
            return Ok(Vec::new());
        }

        let mut lo: u64 = 0;
        let mut hi: u64 = file_size.saturating_sub(1);
        let mut last_lo_below: u64 = 0;
        let mut probe_buf = vec![0u8; 8192];

        for _ in 0..64 {
            if lo >= hi {
                break;
            }
            let mid = lo + (hi - lo) / 2;
            reader.seek(SeekFrom::Start(mid))?;
            let n = reader.read(&mut probe_buf)?;
            if n < JOINED_PREVOUT_HEADER_LEN {
                break;
            }
            let Some((record, _end)) = Self::parse_record_at(&probe_buf[..n], 0) else {
                hi = mid.saturating_sub(1);
                continue;
            };
            if record.spending_block < target_height {
                last_lo_below = mid;
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }

        // Scan forward from the last bracket known to be below target (handles gaps / misalignment).
        reader.seek(SeekFrom::Start(last_lo_below))?;
        let mut leftover = Vec::new();
        let mut read_buf = vec![0u8; 256 * 1024];
        let n = reader.read(&mut read_buf)?;
        if n > 0 {
            leftover.extend_from_slice(&read_buf[..n]);
            Self::align_leftover_prefix(&mut leftover)?;
        }
        loop {
            while leftover.len() >= JOINED_PREVOUT_HEADER_LEN {
                if let Some((record, consumed)) = JoinedPrevout::from_bytes(&leftover) {
                    if record.spending_block >= target_height {
                        return Ok(leftover);
                    }
                    leftover.drain(..consumed);
                } else {
                    // Incomplete record at buffer tail — read more; never drop bytes here.
                    break;
                }
            }
            let n = reader.read(&mut read_buf)?;
            if n == 0 {
                return Ok(leftover);
            }
            leftover.extend_from_slice(&read_buf[..n]);
        }
    }

    /// Skip forward to prevouts for a specific block height
    /// This is needed when resuming from a specific block
    pub fn skip_to_block(&mut self, target_height: u32) -> Result<()> {
        println!("  Seeking prevouts to block {target_height}...");
        let start_time = std::time::Instant::now();

        self.leftover = Self::binary_seek_to_block(&mut self.reader, target_height)?;
        if let Some((record, _)) = Self::parse_record_at(&self.leftover, 0) {
            if record.spending_block >= target_height {
                // Gap in joined_sorted.bin (e.g. 895977–912703): overshoot lands in a later run.
                if record.spending_block > target_height.saturating_add(1000) {
                    self.leftover.clear();
                    println!(
                        "  ✅ No prevouts for block {} (next indexed spending block {}) ({:.2}s)",
                        target_height,
                        record.spending_block,
                        start_time.elapsed().as_secs_f64()
                    );
                    return Ok(());
                }
                println!(
                    "  ✅ Seek to block {} ({:.2}s)",
                    target_height,
                    start_time.elapsed().as_secs_f64()
                );
                return Ok(());
            }
        }

        // Fallback: linear scan (corrupt/padded file edge cases).
        println!("  Binary seek missed target; falling back to linear scan...");
        let mut skipped_records = 0u64;
        let mut last_reported_block = 0u32;

        // Read records until we find one >= target_height
        loop {
            // Try to parse from leftover first
            if self.leftover.len() >= JOINED_PREVOUT_HEADER_LEN {
                if let Some((prevout, consumed)) = JoinedPrevout::from_bytes(&self.leftover) {
                    if prevout.spending_block >= target_height {
                        // Found it - put it back in leftover for next read_block_prevouts
                        let elapsed = start_time.elapsed();
                        println!(
                            "  ✅ Skipped to block {} ({} records in {:.1}s, {:.0} rec/s)",
                            target_height,
                            skipped_records,
                            elapsed.as_secs_f64(),
                            skipped_records as f64 / elapsed.as_secs_f64().max(0.001)
                        );
                        return Ok(());
                    }
                    // This prevout is for an earlier block - skip it
                    self.leftover.drain(..consumed);
                    skipped_records += 1;

                    // Progress reporting every 10k records or every 5k blocks
                    if skipped_records % 10_000 == 0
                        || (prevout.spending_block > last_reported_block + 5_000)
                    {
                        let elapsed = start_time.elapsed();
                        let rate = skipped_records as f64 / elapsed.as_secs_f64();
                        println!(
                            "  ⏩ Skipped {} records (at block {}, {:.0} rec/s, {:.1}s elapsed)",
                            skipped_records,
                            prevout.spending_block,
                            rate,
                            elapsed.as_secs_f64()
                        );
                        last_reported_block = prevout.spending_block;
                    }
                    continue;
                }
            }

            // Read more data
            let n = self.reader.read(&mut self.buffer)?;
            if n == 0 {
                // EOF - no more prevouts, we've passed the target
                let elapsed = start_time.elapsed();
                eprintln!("  ⚠️  Warning: Reached EOF in prevout file before target block {} (skipped {} records in {:.1}s)", 
                    target_height, skipped_records, elapsed.as_secs_f64());
                return Ok(());
            }
            self.leftover.extend_from_slice(&self.buffer[..n]);
        }
    }

    /// Read prevouts for a specific block
    /// Returns prevouts sorted by (tx_idx, input_idx)
    pub fn read_block_prevouts(&mut self, block_height: u32) -> Result<Vec<JoinedPrevout>> {
        let mut prevouts = Vec::new();

        loop {
            // Try to parse from leftover
            while self.leftover.len() >= JOINED_PREVOUT_HEADER_LEN {
                if let Some((prevout, consumed)) = JoinedPrevout::from_bytes(&self.leftover) {
                    if prevout.spending_block < block_height {
                        anyhow::bail!(
                            "PrevoutReader desync at block {}: read spending_block {} (stream went backwards)",
                            block_height, prevout.spending_block
                        );
                    }

                    if prevout.spending_block > block_height {
                        // This prevout is for a future block - don't consume it
                        return Ok(prevouts);
                    }

                    // This prevout is for our block
                    prevouts.push(prevout);
                    self.leftover.drain(..consumed);
                } else {
                    // Incomplete record at buffer tail — read more; never drop bytes here.
                    break;
                }
            }

            // Read more data
            let n = self.reader.read(&mut self.buffer)?;
            if n == 0 {
                return Ok(prevouts); // EOF
            }
            self.leftover.extend_from_slice(&self.buffer[..n]);
        }
    }
}

/// Validate `PrevoutReader` binary seek + sequential read (step6 path).
/// 1. Two independent seek+read passes must agree.
/// 2. Spot-check prevouts called out in v2 false-positive failures.log.
pub fn smoke_test_prevout_reader(
    prevouts_file: &Path,
    start_height: u32,
    end_height: u32,
) -> Result<()> {
    println!(
        "PrevoutReader smoke: blocks {}..={} on {}",
        start_height,
        end_height,
        prevouts_file.display()
    );

    let read_range = |reader: &mut PrevoutReader| -> Result<HashMap<u32, u32>> {
        let mut counts = HashMap::new();
        for height in start_height..=end_height {
            counts.insert(height, reader.read_block_prevouts(height)?.len() as u32);
        }
        Ok(counts)
    };

    let mut a = PrevoutReader::new(prevouts_file)?;
    a.skip_to_block(start_height)?;
    let counts_a = read_range(&mut a)?;

    let mut b = PrevoutReader::new(prevouts_file)?;
    b.skip_to_block(start_height)?;
    let counts_b = read_range(&mut b)?;

    let mut mismatches = 0u32;
    for height in start_height..=end_height {
        let na = counts_a.get(&height).copied().unwrap_or(0);
        let nb = counts_b.get(&height).copied().unwrap_or(0);
        if na != nb {
            mismatches += 1;
            eprintln!("  MISMATCH block {height}: pass_a={na} pass_b={nb}");
        }
    }
    if mismatches > 0 {
        anyhow::bail!(
            "PrevoutReader smoke failed: {} / {} blocks differ between seek passes",
            mismatches,
            end_height - start_height + 1
        );
    }
    println!(
        "  OK: {} blocks — two seek passes agree",
        end_height - start_height + 1
    );

    // Spot-check: v2 run logged missing prevout at 812367 tx 1744 in 6 — must be present.
    if start_height <= 812_367 && end_height >= 812_367 {
        let mut spot = PrevoutReader::new(prevouts_file)?;
        spot.skip_to_block(812_367)?;
        let prevouts = spot.read_block_prevouts(812_367)?;
        let found = prevouts
            .iter()
            .any(|p| p.spending_tx_idx == 1744 && p.spending_input_idx == 6);
        if !found {
            anyhow::bail!(
                "PrevoutReader smoke: block 812367 tx 1744 input 6 not found (v2 false missing)"
            );
        }
        println!("  OK: block 812367 tx 1744 input 6 prevout present");
    }

    Ok(())
}

/// Per-tx prevout data and precomputed hashes, shared across all inputs of the tx via Arc.
struct TxVerifyData {
    values: Vec<i64>,
    scripts: Vec<Vec<u8>>,
    witnesses: Option<Vec<Witness>>,
    tx_flags: u32,
    bip143: Option<blvm_protocol::transaction_hash::Bip143PrecomputedHashes>,
}

/// One block's worth of pre-built verification work, ready for par_iter.
struct LoadedBlock {
    height: u64,
    median_time_past: Option<u64>,
    block: blvm_protocol::types::Block,
    /// (tx_idx, input_idx) task list — only non-coinbase inputs with all prevouts present
    tasks: Vec<(usize, usize)>,
    tx_data: Vec<Option<Arc<TxVerifyData>>>, // indexed by tx_idx; None = coinbase or missing prevout
    /// Pre-formatted missing-prevout error messages
    missing: Vec<(u64, String)>,
}

/// Result of step-6-style verification for a single block (used by `find_error_in_block`).
#[derive(Debug, Default)]
pub struct BlockVerifyResult {
    pub per_input_false: Vec<(usize, usize)>,
    pub schnorr_batch_invalid: usize,
    pub schnorr_batch_total: usize,
    pub missing_prevouts: usize,
    /// Inputs that fail immediate Schnorr verify among step-6 tasks (batch failure locate).
    pub schnorr_failures: Vec<(usize, usize)>,
}

/// Verify one block the same way step 6 does (skip txs with any missing prevout; batch Schnorr).
pub fn verify_block_like_step6(
    height: u64,
    block_data: Vec<u8>,
    block_prevouts: Vec<JoinedPrevout>,
    network: Network,
) -> Result<BlockVerifyResult> {
    let block_header = deserialize_block_header(&block_data[..80.min(block_data.len())])?;
    let median_time_past = Some(get_median_time_past(&[block_header]));
    let lb = build_loaded_block(
        height,
        block_data,
        block_prevouts,
        median_time_past,
        network,
    )?;

    let mut out = BlockVerifyResult {
        missing_prevouts: lb.missing.len(),
        ..Default::default()
    };

    #[cfg(feature = "production")]
    let schnorr_collector = Arc::new(SchnorrSignatureCollector::new());
    #[cfg(not(feature = "production"))]
    let schnorr_collector: Option<Arc<SchnorrSignatureCollector>> = None;

    let per_input_results: Vec<(bool, usize, usize)> = lb
        .tasks
        .par_iter()
        .map(|&(tx_idx, input_idx)| {
            let data = lb.tx_data[tx_idx].as_ref().unwrap();
            let tx = &lb.block.transactions[tx_idx];
            let input = &tx.inputs[input_idx];
            let prevout_script: &[u8] = &data.scripts[input_idx];
            let witness_stack: Option<&Witness> = data
                .witnesses
                .as_ref()
                .and_then(|wits| wits.get(input_idx))
                .filter(|w| !is_witness_empty(w));
            let prevout_script_pubkeys: Vec<&[u8]> =
                data.scripts.iter().map(|s| s.as_slice()).collect();

            let ok = match verify_script_with_context_full(
                &input.script_sig,
                prevout_script,
                witness_stack,
                data.tx_flags,
                tx,
                input_idx,
                &data.values,
                &prevout_script_pubkeys,
                Some(lb.height),
                lb.median_time_past,
                network,
                SigVersion::Base,
                #[cfg(feature = "production")]
                Some(schnorr_collector.as_ref()),
                #[cfg(not(feature = "production"))]
                None,
                data.bip143.as_ref(),
                None,
                None,
                None,
            ) {
                Ok(true) => true,
                Ok(false) => false,
                Err(e) => {
                    eprintln!("  [Err] block={height} tx={tx_idx} in={input_idx} err={e:?}");
                    false
                }
            };
            (ok, tx_idx, input_idx)
        })
        .collect();

    for (ok, tx_idx, input_idx) in per_input_results {
        if !ok {
            out.per_input_false.push((tx_idx, input_idx));
        }
    }

    #[cfg(feature = "production")]
    {
        match schnorr_collector.verify_batch() {
            Ok(batch) => {
                out.schnorr_batch_total = batch.len();
                out.schnorr_batch_invalid = batch.iter().filter(|&&v| !v).count();
                if out.schnorr_batch_invalid > 0 {
                    for &(tx_idx, input_idx) in &lb.tasks {
                        let data = lb.tx_data[tx_idx].as_ref().unwrap();
                        let tx = &lb.block.transactions[tx_idx];
                        let input = &tx.inputs[input_idx];
                        let witness_stack: Option<&Witness> = data
                            .witnesses
                            .as_ref()
                            .and_then(|wits| wits.get(input_idx))
                            .filter(|w| !is_witness_empty(w));
                        let prevout_script_pubkeys: Vec<&[u8]> =
                            data.scripts.iter().map(|s| s.as_slice()).collect();
                        if verify_script_with_context_full(
                            &input.script_sig,
                            &data.scripts[input_idx],
                            witness_stack,
                            data.tx_flags,
                            tx,
                            input_idx,
                            &data.values,
                            &prevout_script_pubkeys,
                            Some(lb.height),
                            lb.median_time_past,
                            network,
                            SigVersion::Base,
                            None,
                            data.bip143.as_ref(),
                            None,
                            None,
                            None,
                        ) == Ok(false)
                        {
                            out.schnorr_failures.push((tx_idx, input_idx));
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("  Schnorr batch error at block {}: {:?}", height, e);
                out.schnorr_batch_invalid = 1;
            }
        }
    }

    Ok(out)
}

/// Build one block's worth of verification work on the calling thread (the loader thread).
fn build_loaded_block(
    height: u64,
    block_data: Vec<u8>,
    block_prevouts: Vec<JoinedPrevout>,
    median_time_past: Option<u64>,
    network: Network,
) -> Result<LoadedBlock> {
    use blvm_protocol::types::{OutPoint, TransactionOutput};

    let (block, witnesses) = deserialize_block_with_witnesses(&block_data)
        .with_context(|| format!("Failed to deserialize block {height}"))?;

    let base_flags = {
        let f = calculate_base_script_flags_for_block_network(height, network);
        let activation = ForkActivationTable::from_network(network);
        let has_taproot = activation.is_fork_active(ForkId::Taproot, height);
        if has_taproot {
            f | 0x20000
        } else {
            f
        }
    };
    let activation = ForkActivationTable::from_network(network);
    let height_has_segwit = activation.is_fork_active(ForkId::SegWit, height);
    let height_has_taproot = activation.is_fork_active(ForkId::Taproot, height);

    // Index prevouts by (tx_idx, input_idx)
    let mut prevout_map: HashMap<(u32, u32), &JoinedPrevout> =
        HashMap::with_capacity(block_prevouts.len());
    for p in &block_prevouts {
        prevout_map.insert((p.spending_tx_idx, p.spending_input_idx), p);
    }

    // Lazy intra-block UTXO index
    let mut intra_block_utxos: HashMap<OutPoint, TransactionOutput> = HashMap::new();
    let mut intra_built = false;

    let mut tx_data: Vec<Option<Arc<TxVerifyData>>> = Vec::with_capacity(block.transactions.len());
    let mut tasks: Vec<(usize, usize)> = Vec::new();
    let mut missing: Vec<(u64, String)> = Vec::new();

    for (tx_idx, tx) in block.transactions.iter().enumerate() {
        if is_coinbase(tx) {
            tx_data.push(None);
            continue;
        }

        let tx_witnesses = witnesses.get(tx_idx);

        let mut values: Vec<i64> = Vec::with_capacity(tx.inputs.len());
        let mut scripts: Vec<Vec<u8>> = Vec::with_capacity(tx.inputs.len());
        let mut has_missing = false;

        for (i, input) in tx.inputs.iter().enumerate() {
            if let Some(p) = prevout_map.get(&(tx_idx as u32, i as u32)) {
                values.push(p.value);
                scripts.push(p.script_pubkey.clone());
            } else {
                if !intra_built {
                    use blvm_protocol::block::calculate_tx_id;
                    for btx in block.transactions.iter() {
                        let txid = calculate_tx_id(btx);
                        for (oi, o) in btx.outputs.iter().enumerate() {
                            intra_block_utxos.insert(
                                OutPoint {
                                    hash: txid,
                                    index: oi as u32,
                                },
                                TransactionOutput {
                                    value: o.value,
                                    script_pubkey: o.script_pubkey.clone(),
                                },
                            );
                        }
                    }
                    intra_built = true;
                }
                if let Some(o) = intra_block_utxos.get(&input.prevout) {
                    values.push(o.value);
                    scripts.push(o.script_pubkey.clone());
                } else {
                    has_missing = true;
                    values.push(0);
                    scripts.push(vec![]);
                }
            }
        }

        if has_missing {
            for (input_idx, input) in tx.inputs.iter().enumerate() {
                if scripts[input_idx].is_empty() && values[input_idx] == 0 {
                    missing.push((
                        height,
                        format!(
                            "Missing prevout: tx {}, input {} (txid: {}, idx: {})",
                            tx_idx,
                            input_idx,
                            hex::encode(input.prevout.hash),
                            input.prevout.index
                        ),
                    ));
                }
            }
            tx_data.push(None);
            continue;
        }

        // Per-tx flags
        let mut tx_flags = base_flags;
        let has_real_witness =
            tx_has_nonempty_input_witness(tx_witnesses.as_ref().map(|w| w.as_slice()));
        if height_has_segwit && tx_requires_witness_script_flags(tx, has_real_witness) {
            tx_flags |= 0x800;
        }
        if height_has_taproot {
            use blvm_protocol::constants::TAPROOT_SCRIPT_LENGTH;
            for o in &tx.outputs {
                let s = &o.script_pubkey;
                if s.len() == TAPROOT_SCRIPT_LENGTH
                    && s[0] == blvm_protocol::opcodes::OP_1
                    && s[1] == blvm_protocol::opcodes::PUSH_32_BYTES
                {
                    tx_flags |= 0x8000;
                    break;
                }
            }
        }

        // Pre-compute BIP143 hashes once per tx (shared across all inputs).
        // Without this, verify_script_with_context_full recomputes 3 double-SHA256s
        // for every single input — O(N) times instead of O(1) for a tx with N inputs.
        let bip143 = if height_has_segwit && has_real_witness {
            Some(blvm_protocol::transaction_hash::Bip143PrecomputedHashes::compute(tx, &[], &[]))
        } else {
            None
        };

        let witnesses_owned: Option<Vec<Witness>> = tx_witnesses.cloned();

        for input_idx in 0..tx.inputs.len() {
            tasks.push((tx_idx, input_idx));
        }

        tx_data.push(Some(Arc::new(TxVerifyData {
            values,
            scripts,
            witnesses: witnesses_owned,
            tx_flags,
            bip143,
        })));
    }

    Ok(LoadedBlock {
        height,
        median_time_past,
        block,
        tasks,
        tx_data,
        missing,
    })
}

/// Verify all scripts in the blockchain using streamed prevout data
pub fn verify_scripts(
    chunks_dir: &Path,
    prevouts_file: &Path,
    start_height: u64,
    end_height: u64,
    progress_interval: u64,
    network: Network,
) -> Result<(u64, u64, Vec<(u64, String)>)> {
    println!("\n{}", "═".repeat(60));
    println!("STEP 6: Parallel Script Verification");
    println!("{}", "═".repeat(60));
    println!("  Chunks dir: {}", chunks_dir.display());
    println!("  Blocks: {start_height} to {end_height}");
    println!("  Prevouts: {}", prevouts_file.display());
    println!("  Using {} threads", rayon::current_num_threads());

    let start_time = Instant::now();

    // Create block iterator and prevout reader.
    let mut block_iter = ChunkedBlockIterator::new(chunks_dir, Some(start_height), None)?
        .ok_or_else(|| {
            anyhow::anyhow!("Failed to create block iterator - chunks.meta not found?")
        })?;
    let mut prevout_reader = PrevoutReader::new(prevouts_file)?;

    if start_height > 0 {
        println!("  Skipping prevouts to block {start_height}...");
        prevout_reader.skip_to_block(start_height as u32)?;
        println!("  ✅ Skipped to block {start_height}");
    }

    // Failure log
    let failures_file = prevouts_file
        .parent()
        .unwrap_or(Path::new("."))
        .join("failures.log");
    let mut failures_writer = BufWriter::new(
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&failures_file)?,
    );
    writeln!(
        failures_writer,
        "# Block Height | Error Type | Details | TX Hex"
    )?;

    // ── Pipeline: loader thread + verify loop ──────────────────────────────────
    // The bottleneck is that loading+decompressing blocks from the 104 GB chunk
    // serialises with script verification, leaving 31/32 rayon workers idle.
    // A bounded channel with 4-block lookahead fully overlaps I/O and CPU work.
    let (tx_chan, rx_chan) = std::sync::mpsc::sync_channel::<Result<LoadedBlock>>(4);

    // Loader thread: reads raw block bytes, reads prevouts, computes median_time_past,
    // deserialises, and pre-builds TxVerifyData (incl. BIP143 hashes) — all sequentially.
    let _chunks_dir_owned = chunks_dir.to_path_buf();
    let loader_network = network;
    std::thread::spawn(move || {
        let mut recent_headers: VecDeque<BlockHeader> = VecDeque::with_capacity(12);
        let mut height = start_height;

        loop {
            if height >= end_height {
                break;
            }

            let block_data = match block_iter.next_block() {
                Ok(Some(d)) => d,
                Ok(None) => break,
                Err(e) => {
                    let _ = tx_chan.send(Err(e.context(format!("loading block {height}"))));
                    return;
                }
            };

            if block_data.len() < 80 {
                let _ = tx_chan.send(Err(anyhow::anyhow!(
                    "Block {} too short ({} bytes)",
                    height,
                    block_data.len()
                )));
                return;
            }

            let block_header = match deserialize_block_header(&block_data[..80]) {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx_chan.send(Err(anyhow::anyhow!("header {}: {:?}", height, e)));
                    return;
                }
            };

            // BIP113: median of the 11 headers BEFORE this block
            let median_time_past = if recent_headers.is_empty() {
                None
            } else {
                let s = recent_headers.make_contiguous();
                Some(get_median_time_past(s))
            };
            recent_headers.push_back(block_header);
            if recent_headers.len() > 11 {
                recent_headers.pop_front();
            }

            let block_prevouts = match prevout_reader.read_block_prevouts(height as u32) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx_chan.send(Err(e.context(format!("prevouts {height}"))));
                    return;
                }
            };

            let loaded = build_loaded_block(
                height,
                block_data,
                block_prevouts,
                median_time_past,
                loader_network,
            );
            if tx_chan.send(loaded).is_err() {
                return; // receiver hung up
            }
            height += 1;
        }
        drop(tx_chan);
    });

    // ── Verify loop (main thread) ───────────────────────────────────────────────
    let total_verified = Arc::new(AtomicU64::new(0));
    let total_failed = Arc::new(AtomicU64::new(0));
    let mut divergences: Vec<(u64, String)> = Vec::new();
    let mut failure_stats: HashMap<String, u64> = HashMap::new();
    failure_stats.insert("Missing prevout".to_string(), 0);
    failure_stats.insert("Script returned false".to_string(), 0);
    failure_stats.insert("Script error".to_string(), 0);

    let mut last_report = Instant::now();
    let mut sample_counter = 0u64;
    let mut height = start_height;

    for loaded in rx_chan {
        let lb = loaded?;
        height = lb.height;

        // Count and log missing-prevout failures (pre-built by loader thread)
        for (h, msg) in &lb.missing {
            total_failed.fetch_add(1, Ordering::Relaxed);
            *failure_stats
                .entry("Missing prevout".to_string())
                .or_insert(0) += 1;
            sample_counter += 1;
            let should_log = sample_counter % 1000 == 0
                || sample_counter <= 1000
                || h % 10000 == 0
                || h % 5000 == 0 && *h > 400_000;
            if should_log {
                writeln!(failures_writer, "{h} | Missing prevout | {msg}")?;
                failures_writer.flush()?;
            }
        }

        // Parallel script verification.
        // Per-block Schnorr collector → blvm-secp256k1 batch verify (same as connect_block).
        // Without this, verify_script_with_context_full verifies each Taproot sig individually.
        // Use new() (unbounded SegQueue) rather than new_with_capacity: a block's Schnorr sig
        // count can exceed its input count when tapscripts have multiple OP_CHECKSIG operations.
        #[cfg(feature = "production")]
        let schnorr_collector = Arc::new(SchnorrSignatureCollector::new());
        #[cfg(not(feature = "production"))]
        let schnorr_collector: Option<Arc<SchnorrSignatureCollector>> = None;

        let results: Vec<(bool, usize, usize, u8)> = lb
            .tasks
            .par_iter()
            .map(|&(tx_idx, input_idx)| {
                let data = lb.tx_data[tx_idx].as_ref().unwrap();
                let tx = &lb.block.transactions[tx_idx];
                let input = &tx.inputs[input_idx];
                let prevout_script: &[u8] = &data.scripts[input_idx];

                let witness_stack: Option<&Witness> = data
                    .witnesses
                    .as_ref()
                    .and_then(|wits| wits.get(input_idx))
                    .filter(|w| !is_witness_empty(w));

                let prevout_script_pubkeys: Vec<&[u8]> =
                    data.scripts.iter().map(|s| s.as_slice()).collect();

                let r = verify_script_with_context_full(
                    &input.script_sig,
                    prevout_script,
                    witness_stack,
                    data.tx_flags,
                    tx,
                    input_idx,
                    &data.values,
                    &prevout_script_pubkeys,
                    Some(lb.height),
                    lb.median_time_past,
                    network,
                    SigVersion::Base,
                    #[cfg(feature = "production")]
                    Some(schnorr_collector.as_ref()),
                    #[cfg(not(feature = "production"))]
                    None,
                    data.bip143.as_ref(),
                    None, // precomputed_sighash_all
                    None, // sighash_cache
                    None, // precomputed_p2pkh_hash
                );
                match r {
                    Ok(true) => (true, tx_idx, input_idx, 0u8),
                    Ok(false) => (false, tx_idx, input_idx, 1u8),
                    Err(ref e) => {
                        eprintln!(
                            "  [Err] block={} tx={} in={} err={:?}",
                            lb.height, tx_idx, input_idx, e
                        );
                        (false, tx_idx, input_idx, 2u8)
                    }
                }
            })
            .collect();

        // Batch-verify deferred Schnorr signatures collected during par_iter above.
        #[cfg(feature = "production")]
        {
            match schnorr_collector.verify_batch() {
                Ok(batch) if batch.iter().any(|&v| !v) => {
                    let bad = batch.iter().filter(|&&v| !v).count() as u64;
                    total_failed.fetch_add(bad, Ordering::Relaxed);
                    *failure_stats
                        .entry("Script returned false".to_string())
                        .or_insert(0) += bad;
                    sample_counter += bad;
                    let msg = format!(
                        "Schnorr batch verification: {bad}/{} invalid at block {}",
                        batch.len(),
                        height
                    );
                    writeln!(
                        failures_writer,
                        "{} | Script returned false | {} |",
                        height, msg
                    )?;
                    failures_writer.flush()?;
                    if divergences.len() < 100 {
                        divergences.push((height, msg));
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    total_failed.fetch_add(1, Ordering::Relaxed);
                    *failure_stats.entry("Script error".to_string()).or_insert(0) += 1;
                    let msg = format!("Schnorr batch error at block {}: {:?}", height, e);
                    writeln!(failures_writer, "{} | Script error | {} |", height, msg)?;
                    failures_writer.flush()?;
                }
            }
        }

        // Process results
        for (success, tx_idx, input_idx, tag) in results {
            if success {
                total_verified.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let failure_type = match tag {
                1 => "Script returned false",
                _ => "Script error",
            };
            total_failed.fetch_add(1, Ordering::Relaxed);
            *failure_stats.entry(failure_type.to_string()).or_insert(0) += 1;

            let msg = format!("{tx_idx}:{input_idx}");
            let is_error = tag == 2;
            sample_counter += 1;
            let should_log = is_error
                || sample_counter % 1000 == 0
                || sample_counter <= 1000
                || height % 10000 == 0
                || height % 5000 == 0 && height > 400_000;

            if should_log {
                let err_detail = if is_error {
                    // Re-run is too expensive; Err was logged to stderr during par_iter.
                    format!("Script error: tx {tx_idx}, input {input_idx}")
                } else {
                    format!("Script returned false: tx {tx_idx}, input {input_idx}")
                };
                let tx_hex = hex::encode(serialize_transaction(&lb.block.transactions[tx_idx]));
                writeln!(
                    failures_writer,
                    "{height} | {failure_type} | {err_detail} | {tx_hex}"
                )?;
                failures_writer.flush()?;
            }
            if divergences.len() < 100 {
                divergences.push((height, msg));
            }
        }

        // Progress
        let processed = height - start_height + 1;
        if processed % progress_interval == 0
            || last_report.elapsed().as_secs() >= 10
            || height < start_height + 100
        {
            let elapsed = start_time.elapsed().as_secs_f64();
            let rate = processed as f64 / elapsed;
            let remaining = (end_height - height) as f64 / rate;
            let v = total_verified.load(Ordering::Relaxed);
            let f = total_failed.load(Ordering::Relaxed);
            let missing = failure_stats.get("Missing prevout").unwrap_or(&0);
            let script_false = failure_stats.get("Script returned false").unwrap_or(&0);
            let script_err = failure_stats.get("Script error").unwrap_or(&0);
            println!(
                "  Block {}/{} ({:.1}%) - ✓{} ✗{} (M:{} F:{} E:{}) - {:.0} blk/s - ETA: {:.0}m",
                height,
                end_height,
                (height - start_height) as f64 / (end_height - start_height) as f64 * 100.0,
                v,
                f,
                missing,
                script_false,
                script_err,
                rate,
                remaining / 60.0
            );
            last_report = Instant::now();
        }
    }

    let elapsed = start_time.elapsed();
    let verified_final = total_verified.load(Ordering::Relaxed);
    let failed_final = total_failed.load(Ordering::Relaxed);
    let blocks_processed = height.saturating_sub(start_height) + 1;

    failures_writer.flush()?;

    println!("{}", "─".repeat(60));
    println!("  ✅ Step 6 Complete!");
    println!("  Verified: {verified_final}");
    println!("  Failed: {failed_final}");
    println!("  Failure breakdown:");
    for (t, c) in &failure_stats {
        println!(
            "    {}: {} ({:.2}%)",
            t,
            c,
            *c as f64 / failed_final.max(1) as f64 * 100.0
        );
    }
    println!(
        "  Divergences sampled: {} (see {})",
        divergences.len(),
        failures_file.display()
    );
    println!("  Blocks processed: {blocks_processed}");
    println!("  Time: {:.1}m", elapsed.as_secs_f64() / 60.0);
    println!(
        "  Rate: {:.0} blocks/sec",
        blocks_processed as f64 / elapsed.as_secs_f64()
    );

    Ok((verified_final, failed_final, divergences))
}
