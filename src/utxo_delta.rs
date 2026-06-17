//! Delta UTXO checkpoint format.
//!
//! A delta file stores only the UTXOs **added** and the OutPoints **removed** between two
//! consecutive checkpoint heights. Combined with a base snapshot (`utxo_H.bin`), a full UTXO
//! set at any later checkpoint can be reconstructed by applying deltas forward.
//!
//! On-disk layout (`BLVMDL01`):
//! ```text
//! magic:        8 bytes  "BLVMDL01"
//! version:      4 bytes  LE u32 (1)
//! height:       8 bytes  LE u64 (height AFTER applying this delta)
//! base_height:  8 bytes  LE u64 (the checkpoint this delta is relative to)
//! num_added:    8 bytes  LE u64
//! num_removed:  8 bytes  LE u64
//! [added entries]: each = outpoint(36) + value(8) + height(8) + is_coinbase(1) + spk_len(4) + spk(var)
//! [removed entries]: each = outpoint(36)
//! ```
//!
//! ## Memory model
//!
//! `DeltaAccumulator` streams **all** on-disk data (adds AND removes) to temp files.
//! Only one `HashSet<OutPoint>` lives in RAM:
//!
//! - `created_in_interval` — OutPoints streamed to the adds file that are still live
//!   (haven't been spent within the interval yet). When an intra-interval spend happens,
//!   the entry is removed from this set (cancellation). At finalize, the set is the exact
//!   inclusion filter for the adds pass.
//!
//! Removed OutPoints are written directly to a `removed_tmp_H.bin.part` file — no
//! in-memory `HashSet<OutPoint>` for removes.  Valid Bitcoin never double-spends the same
//! UTXO across blocks, so the removed stream needs no deduplication.
//!
//! The BIP30 guard (`!removed.contains(op)` before an add) is intentionally absent: it is
//! only reachable before height 91842 (two known duplicate-txid pairs), well before any
//! interval starting at ≥ 100 000.

use anyhow::{Context, Result, bail};
use blvm_protocol::types::{OutPoint, UTXO, UtxoSet};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const DELTA_MAGIC: &[u8; 8] = b"BLVMDL01";
pub const DELTA_FORMAT_VERSION: u32 = 1;
pub const DELTA_HEADER_LEN: usize = 8 + 4 + 8 + 8 + 8 + 8; // 44 bytes

/// In-memory representation of a delta between two checkpoint heights.
/// Used for *reading* deltas; the writer side uses `DeltaAccumulator`.
pub struct UtxoDelta {
    pub height: u64,
    pub base_height: u64,
    pub added: Vec<(OutPoint, UTXO)>,
    pub removed: Vec<OutPoint>,
}

/// Streaming delta accumulator.
///
/// Both UTXO add-bytes and removed OutPoints are streamed to temp files on disk as each
/// block connects.  Only `created_in_interval: HashSet<OutPoint>` lives in RAM.
pub struct DeltaAccumulator {
    base_height: u64,
    tmp_dir: PathBuf,

    adds_path: PathBuf,
    adds_writer: Option<BufWriter<File>>,
    adds_written: u64,

    removed_path: PathBuf,
    removed_writer: Option<BufWriter<File>>,
    removed_count: u64,

    /// OutPoints in the adds stream that are still live.  Shrinks on intra-interval spends.
    /// At finalize time this is the exact inclusion filter.
    created_in_interval: HashSet<OutPoint>,
}

impl DeltaAccumulator {
    pub fn new(base_height: u64, tmp_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(tmp_dir)
            .with_context(|| format!("create tmp_dir {}", tmp_dir.display()))?;
        let adds_path = tmp_dir.join(format!("adds_tmp_{base_height}.bin.part"));
        let removed_path = tmp_dir.join(format!("removed_tmp_{base_height}.bin.part"));
        let adds_writer = BufWriter::with_capacity(
            2 * 1024 * 1024,
            File::create(&adds_path)
                .with_context(|| format!("create adds file {}", adds_path.display()))?,
        );
        let removed_writer = BufWriter::with_capacity(
            2 * 1024 * 1024,
            File::create(&removed_path)
                .with_context(|| format!("create removed file {}", removed_path.display()))?,
        );
        Ok(Self {
            base_height,
            tmp_dir: tmp_dir.to_owned(),
            adds_path,
            adds_writer: Some(adds_writer),
            adds_written: 0,
            removed_path,
            removed_writer: Some(removed_writer),
            removed_count: 0,
            created_in_interval: HashSet::new(),
        })
    }

    /// Record one block's UTXO changes.  All RAM ops are O(1) amortized; IO streams to disk.
    ///
    /// `spent`   = OutPoints consumed by this block's inputs.
    /// `created` = new UTXOs produced by this block's outputs.
    pub fn record_block(&mut self, spent: &[OutPoint], created: &UtxoSet) -> Result<()> {
        let rw = self
            .removed_writer
            .as_mut()
            .expect("DeltaAccumulator: removed_writer is None");
        for op in spent {
            if !self.created_in_interval.remove(op) {
                // Pre-existing UTXO spent: stream to removed file.
                rw.write_all(&op.hash)?;
                rw.write_all(&op.index.to_le_bytes())?;
                self.removed_count += 1;
            }
            // else: intra-interval cancel — removed from created_in_interval, excluded at finalize.
        }
        let aw = self
            .adds_writer
            .as_mut()
            .expect("DeltaAccumulator: adds_writer is None");
        for (op, utxo) in created.iter() {
            write_utxo_entry(aw, op, utxo)?;
            self.created_in_interval.insert(*op);
            self.adds_written += 1;
        }
        Ok(())
    }

    /// Finalize: write the delta file to `out_path` (atomic rename), then reset for the
    /// next interval starting at `height`.  Safe to call mid-interval for memory-guard exits.
    ///
    /// Returns `(num_added, num_removed)`.
    pub fn finalize_to_file(&mut self, height: u64, out_path: &Path) -> Result<(u64, u64)> {
        if let Some(mut w) = self.adds_writer.take() {
            w.flush()?;
        }
        if let Some(mut w) = self.removed_writer.take() {
            w.flush()?;
        }

        let num_added = self.created_in_interval.len() as u64;
        let num_removed = self.removed_count;

        let tmp_out = out_path.with_extension("delta.part");
        {
            let mut w = BufWriter::with_capacity(
                2 * 1024 * 1024,
                File::create(&tmp_out).with_context(|| format!("create {}", tmp_out.display()))?,
            );
            w.write_all(DELTA_MAGIC)?;
            w.write_all(&DELTA_FORMAT_VERSION.to_le_bytes())?;
            w.write_all(&height.to_le_bytes())?;
            w.write_all(&self.base_height.to_le_bytes())?;
            w.write_all(&num_added.to_le_bytes())?;
            w.write_all(&num_removed.to_le_bytes())?;

            // Adds: stream from temp file, include only entries in created_in_interval.
            {
                let mut r = BufReader::with_capacity(
                    2 * 1024 * 1024,
                    File::open(&self.adds_path)
                        .with_context(|| format!("open {}", self.adds_path.display()))?,
                );
                for _ in 0..self.adds_written {
                    let (op, utxo) = read_utxo_entry(&mut r)?;
                    if self.created_in_interval.contains(&op) {
                        write_utxo_entry(&mut w, &op, &utxo)?;
                    }
                }
            }

            // Removes: stream verbatim from temp file (no dedup needed for valid Bitcoin).
            {
                let mut r = BufReader::with_capacity(
                    2 * 1024 * 1024,
                    File::open(&self.removed_path)
                        .with_context(|| format!("open {}", self.removed_path.display()))?,
                );
                let mut buf = [0u8; 36];
                for _ in 0..self.removed_count {
                    r.read_exact(&mut buf)?;
                    w.write_all(&buf)?;
                }
            }

            w.flush()?;
        }

        std::fs::rename(&tmp_out, out_path)
            .with_context(|| format!("rename {} → {}", tmp_out.display(), out_path.display()))?;
        let mut perms = std::fs::metadata(out_path)?.permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(out_path, perms)?;

        let _ = std::fs::remove_file(&self.adds_path);
        let _ = std::fs::remove_file(&self.removed_path);

        self.reset_state(height)?;
        Ok((num_added, num_removed))
    }

    /// Reset for a new interval starting at `new_base_height` (called after a full base snapshot).
    pub fn reset(&mut self, new_base_height: u64) -> Result<()> {
        if let Some(mut w) = self.adds_writer.take() {
            w.flush()?;
        }
        if let Some(mut w) = self.removed_writer.take() {
            w.flush()?;
        }
        let _ = std::fs::remove_file(&self.adds_path);
        let _ = std::fs::remove_file(&self.removed_path);
        self.reset_state(new_base_height)
    }

    fn reset_state(&mut self, new_base_height: u64) -> Result<()> {
        let adds_path = self
            .tmp_dir
            .join(format!("adds_tmp_{new_base_height}.bin.part"));
        let removed_path = self
            .tmp_dir
            .join(format!("removed_tmp_{new_base_height}.bin.part"));
        self.adds_writer = Some(BufWriter::with_capacity(
            2 * 1024 * 1024,
            File::create(&adds_path).with_context(|| format!("create {}", adds_path.display()))?,
        ));
        self.removed_writer = Some(BufWriter::with_capacity(
            2 * 1024 * 1024,
            File::create(&removed_path)
                .with_context(|| format!("create {}", removed_path.display()))?,
        ));
        self.adds_path = adds_path;
        self.removed_path = removed_path;
        self.base_height = new_base_height;
        self.adds_written = 0;
        self.removed_count = 0;
        self.created_in_interval.clear();
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.adds_written == 0 && self.removed_count == 0
    }

    /// `(net_adds_so_far, removes_so_far)` for progress logging.
    pub fn stats(&self) -> (usize, usize) {
        (self.created_in_interval.len(), self.removed_count as usize)
    }

    pub fn base_height(&self) -> u64 {
        self.base_height
    }
}

impl Drop for DeltaAccumulator {
    fn drop(&mut self) {
        self.adds_writer.take();
        self.removed_writer.take();
        let _ = std::fs::remove_file(&self.adds_path);
        let _ = std::fs::remove_file(&self.removed_path);
    }
}

/// Read a delta file from disk into memory (for reconstruction / apply).
pub fn read_delta(path: &Path) -> Result<UtxoDelta> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut r = BufReader::with_capacity(1024 * 1024, file);

    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if magic != *DELTA_MAGIC {
        bail!("not a delta file: bad magic in {}", path.display());
    }
    let mut u4 = [0u8; 4];
    r.read_exact(&mut u4)?;
    let version = u32::from_le_bytes(u4);
    if version != DELTA_FORMAT_VERSION {
        bail!("unsupported delta version {version} in {}", path.display());
    }

    let mut u8buf = [0u8; 8];
    r.read_exact(&mut u8buf)?;
    let height = u64::from_le_bytes(u8buf);
    r.read_exact(&mut u8buf)?;
    let base_height = u64::from_le_bytes(u8buf);
    r.read_exact(&mut u8buf)?;
    let num_added = u64::from_le_bytes(u8buf);
    r.read_exact(&mut u8buf)?;
    let num_removed = u64::from_le_bytes(u8buf);

    let mut added = Vec::with_capacity(num_added.min(10_000_000) as usize);
    for _ in 0..num_added {
        let (op, utxo) = read_utxo_entry(&mut r)?;
        added.push((op, utxo));
    }

    let mut removed = Vec::with_capacity(num_removed.min(50_000_000) as usize);
    for _ in 0..num_removed {
        let mut hash = [0u8; 32];
        r.read_exact(&mut hash)?;
        r.read_exact(&mut u4)?;
        let index = u32::from_le_bytes(u4);
        removed.push(OutPoint {
            hash: hash.into(),
            index,
        });
    }

    Ok(UtxoDelta {
        height,
        base_height,
        added,
        removed,
    })
}

/// Apply a delta to a mutable UTXO set in memory.
pub fn apply_delta(set: &mut UtxoSet, delta: &UtxoDelta) {
    for op in &delta.removed {
        set.remove(op);
    }
    for (op, utxo) in &delta.added {
        set.insert(*op, Arc::new(utxo.clone()));
    }
}

/// Load a base snapshot then apply a sequence of delta files to reconstruct the UTXO set.
pub fn load_base_and_apply_deltas(
    base_path: &Path,
    delta_paths: &[&Path],
) -> Result<(u64, UtxoSet)> {
    let mut set = crate::utxo_snapshot_fixed_v1::decode_fixed_v1_file(base_path)?;
    let mut current_height = crate::utxo_snapshot_fixed_v1::read_header_height(base_path)?;

    for dp in delta_paths {
        let delta = read_delta(dp)?;
        if delta.base_height != current_height {
            bail!(
                "delta chain broken: expected base_height={current_height}, got {} in {}",
                delta.base_height,
                dp.display()
            );
        }
        apply_delta(&mut set, &delta);
        current_height = delta.height;
    }

    Ok((current_height, set))
}

fn write_utxo_entry<W: Write>(w: &mut W, op: &OutPoint, u: &UTXO) -> Result<()> {
    let script: &[u8] = u.script_pubkey.as_ref();
    w.write_all(&op.hash)?;
    w.write_all(&op.index.to_le_bytes())?;
    w.write_all(&u.value.to_le_bytes())?;
    w.write_all(&u.height.to_le_bytes())?;
    w.write_all(&[u.is_coinbase as u8])?;
    w.write_all(&(script.len() as u32).to_le_bytes())?;
    w.write_all(script)?;
    Ok(())
}

fn read_utxo_entry<R: Read>(r: &mut R) -> Result<(OutPoint, UTXO)> {
    let mut hash = [0u8; 32];
    r.read_exact(&mut hash)?;
    let mut u4 = [0u8; 4];
    r.read_exact(&mut u4)?;
    let index = u32::from_le_bytes(u4);
    let mut u8buf = [0u8; 8];
    r.read_exact(&mut u8buf)?;
    let value = i64::from_le_bytes(u8buf);
    r.read_exact(&mut u8buf)?;
    let height = u64::from_le_bytes(u8buf);
    let mut cb = [0u8; 1];
    r.read_exact(&mut cb)?;
    let is_coinbase = cb[0] != 0;
    r.read_exact(&mut u4)?;
    let spk_len = u32::from_le_bytes(u4) as usize;
    if spk_len > crate::utxo_snapshot_fixed_v1::MAX_SCRIPT_PUBKEY_BYTES {
        bail!("script_pubkey length {spk_len} exceeds max");
    }
    let mut spk = vec![0u8; spk_len];
    r.read_exact(&mut spk)?;
    let op = OutPoint {
        hash: hash.into(),
        index,
    };
    let utxo = UTXO {
        value,
        script_pubkey: spk.into(),
        height,
        is_coinbase,
    };
    Ok((op, utxo))
}
