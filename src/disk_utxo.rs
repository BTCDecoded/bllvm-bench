//! Disk-backed UTXO set via **RocksDB** for memory-constrained checkpoint builds.
//!
//! Architecture: **write-back overlay** in front of RocksDB on SSD.
//!
//! - **Overlay** (`HashMap`): holds recently created outputs and pending deletes. Capped at
//!   `DISK_UTXO_OVERLAY_CAP` entries (default **8M**). Most UTXOs are spent within a few
//!   hundred blocks of creation, so the overlay absorbs the hottest read/write traffic without
//!   hitting the LSM.
//! - **RocksDB**: cold storage for the full UTXO set. Writes use `WriteBatch` with WAL
//!   disabled and no fsync between checkpoint boundaries unless `flush_overlay(true)`.
//!   If the process crashes, reload from the last `.bin` checkpoint.
//!
//! The live row count is kept in key [`META_LEN_KEY`] and updated on every overlay flush so open
//! stays O(1). The UTXO chain tip (last applied block height) is in [`META_TIP_KEY`]. UTXO keys are
//! always 36 bytes (outpoint); meta keys are separate.
//!
//! DB path: **`DISK_UTXO_DB_PATH`** (directory) or **`$XDG_CACHE_HOME/blvm-bench/utxo_rocksdb`**.
//!
//! **Rehydration tuning** (streaming checkpoint → DB):
//! - **`DISK_UTXO_ROCKSDB_CACHE_BYTES`** (or legacy **`DISK_UTXO_REDB_CACHE_BYTES`**): block cache (default **512 MiB**, min env override 64 MiB).
//! - **`DISK_UTXO_REHYDRATE_BATCH`**: rows per `WriteBatch` (default **500_000**). Larger ⇒ fewer writes; more RAM in batch.
//! - **`DISK_UTXO_REHYDRATE_SINGLE_TX`**: `1` = one giant batch (fast; high RAM).
//! - **`DISK_UTXO_REHYDRATE_STATUS_EVERY`**: progress file (default **100_000** rows).
//! - **`DISK_UTXO_REHYDRATE_READ_BUF_MB`**: `BufReader` for `.bin` (default **4**).
//! - **`DISK_UTXO_CHECKPOINT_WRITE_BUF_MB`**: `BufWriter` for DB → `.bin` (default **4**).
//!
//! **Compare-loop / steady-state tuning** (kernel diff at 500k+ height):
//! - **`DISK_UTXO_ROCKSDB_MAX_BACKGROUND_JOBS`**: RocksDB compaction parallelism (default **4**,
//!   range **2–16**). Higher can clear L0 backlog faster on many-core hosts with spare RAM (~130 MB
//!   temporary RAM per active compaction job).
//! - **`DISK_UTXO_WRITE_BUFFER_MB`**: memtable size per CF (default **128**, range **64–512**).
//!   Larger ⇒ fewer L0 files per overlay flush; more write-buffer RAM.
//! - **`DISK_UTXO_OVERLAY_FLUSH_CHUNK_ENTRIES`**: rows per `WriteBatch` slice in `write_flush_batch`
//!   (default **200_000**, range **100_000–500_000**). Larger ⇒ fewer `DB::write` calls per flush.
//! - **`DISK_UTXO_COMPACTION_RATE_LIMIT_MB`**: cap background compaction write I/O in MB/s
//!   (default **0** = unlimited). Useful on shared-disk setups; rarely needed when both Direct I/O
//!   options are active (compaction already isolated from page cache).
//! - **`DISK_UTXO_DIRECT_READS`**: set `1` to enable Direct I/O for reads (default **0** = disabled).
//!   With Direct I/O reads, ALL RocksDB file I/O bypasses the OS page cache, relying on the
//!   internal block cache only.  **In practice, this INCREASES BLVM RSS by ~2 GB** (data leaves
//!   kernel page cache and enters process heap), leaving less total page cache for both DBs.
//!   Only useful on machines with >32 GB RAM where both DBs fit in page cache simultaneously.

use anyhow::{bail, Context, Result};
use blvm_protocol::types::{utxo_set_with_capacity, OutPoint, UtxoSet, UTXO};
use rocksdb::{BlockBasedOptions, Cache, IteratorMode, Options, WriteBatch, WriteOptions, DB};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Key: 8-byte little-endian live UTXO count, updated on every overlay flush.
const META_LEN_KEY: &[u8] = b"__disk_utxo_len__";
/// Key: 8-byte little-endian last-applied block height (UTXO state matches chain through this height).
const META_TIP_KEY: &[u8] = b"__disk_utxo_tip__";

#[inline]
fn is_disk_utxo_meta_key(k: &[u8]) -> bool {
    k == META_LEN_KEY || k == META_TIP_KEY
}

/// Path for **`disk-utxo` rehydration progress** (one line for `kernel-diff-status.sh`).
/// Override: **`CHUNK_UTXO_REHYDRATE_STATUS_FILE`**. Default: `{BLOCK_CACHE_DIR}/.chunk_utxo_rehydrate.status`.
pub fn rehydrate_status_path(cache_root: &Path) -> PathBuf {
    std::env::var_os("CHUNK_UTXO_REHYDRATE_STATUS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| cache_root.join(".chunk_utxo_rehydrate.status"))
}

/// `LOADED TOTAL CHECKPOINT_HEIGHT UNIX_SECS` — atomic replace from `tmp`.
fn write_rehydrate_progress(path: &Path, loaded: u64, total: u64, checkpoint_height: u64) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{loaded} {total} {checkpoint_height} {ts}\n");
    let tmp = path.with_extension("rehydr.tmp");
    if fs::write(&tmp, line).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

pub fn clear_rehydrate_progress(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Path for **`disk-utxo` compact progress** status file.
/// Format: `BYTES_BEFORE UNIX_START` (written at start, deleted on finish).
/// Override: **`CHUNK_UTXO_COMPACT_STATUS_FILE`**. Default: `{BLOCK_CACHE_DIR}/.chunk_utxo_compact.status`.
pub fn compact_status_path(cache_root: &Path) -> PathBuf {
    std::env::var_os("CHUNK_UTXO_COMPACT_STATUS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| cache_root.join(".chunk_utxo_compact.status"))
}

/// One line: last-applied block height (UTXO tip). Same value as [`META_TIP_KEY`] in RocksDB — lets
/// shell resume scripts pick `--start` without opening the DB. Override: **`CHUNK_UTXO_DISK_TIP_FILE`**.
pub fn disk_tip_status_path(cache_root: &Path) -> PathBuf {
    std::env::var_os("CHUNK_UTXO_DISK_TIP_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| cache_root.join(".chunk_utxo_disk_tip"))
}

/// Atomic write of [`disk_tip_status_path`] for external tooling.
pub fn write_disk_tip_status_file(path: &Path, tip: u64) {
    let line = format!("{tip}\n");
    let tmp = path.with_extension("disk_tip.tmp");
    if fs::write(&tmp, line).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn write_compact_status(path: &Path, bytes_before: u64, start_ts: u64) {
    let line = format!("{bytes_before} {start_ts}\n");
    let tmp = path.with_extension("compact.tmp");
    if fs::write(&tmp, line).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn clear_compact_status(path: &Path) {
    let _ = fs::remove_file(path);
}

fn rehydrate_status_every_entries() -> u64 {
    std::env::var("DISK_UTXO_REHYDRATE_STATUS_EVERY")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(100_000)
}

fn rocksdb_max_background_jobs() -> i32 {
    std::env::var("DISK_UTXO_ROCKSDB_MAX_BACKGROUND_JOBS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4)
        .clamp(2, 16)
}

fn rocksdb_write_buffer_bytes() -> usize {
    let mb = std::env::var("DISK_UTXO_WRITE_BUFFER_MB")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(128)
        .clamp(64, 512);
    mb * 1024 * 1024
}

fn overlay_flush_chunk_entries() -> usize {
    std::env::var("DISK_UTXO_OVERLAY_FLUSH_CHUNK_ENTRIES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200_000)
        .clamp(100_000, 500_000)
}

fn rocksdb_cache_bytes() -> usize {
    std::env::var("DISK_UTXO_ROCKSDB_CACHE_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            std::env::var("DISK_UTXO_REDB_CACHE_BYTES")
                .ok()
                .and_then(|s| s.parse().ok())
        })
        .filter(|&n| n >= 64 * 1024 * 1024)
        .unwrap_or(512 * 1024 * 1024)
}

/// Whether to use Direct I/O for all RocksDB reads (default: **false**).
///
/// Direct I/O for reads makes ALL RocksDB SST file reads bypass the OS page cache, relying
/// solely on the RocksDB block cache.  This was intended to free the page cache for Core's
/// LevelDB chainstate, but in practice it INCREASES BLVM's RSS by ~2 GB (data that would
/// otherwise live in kernel page cache now occupies process heap), leaving LESS page cache
/// overall.  Without Direct I/O, BLVM and Core share the full page cache (~6–7 GB on a
/// 16 GB machine with a 7 GB RSS limit), which is more effective than exclusive-but-smaller
/// Core-only cache.  Only enable on machines with >32 GB RAM where both DBs fit simultaneously.
///
/// Override: **`DISK_UTXO_DIRECT_READS`** — set to `1` to enable Direct I/O for reads.
fn rocksdb_use_direct_reads() -> bool {
    match std::env::var("DISK_UTXO_DIRECT_READS").as_deref() {
        Ok("1") | Ok("true") => true,
        _ => false, // default off
    }
}

fn rehydrate_batch_rows() -> u64 {
    std::env::var("DISK_UTXO_REHYDRATE_BATCH")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n >= 10_000)
        .unwrap_or(500_000)
}

fn rehydrate_single_tx() -> bool {
    matches!(
        std::env::var("DISK_UTXO_REHYDRATE_SINGLE_TX").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// Read buffer for streaming checkpoint → DB (MiB). Default 4; clamp 1–256.
fn rehydrate_read_buf_capacity() -> usize {
    std::env::var("DISK_UTXO_REHYDRATE_READ_BUF_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(|mb: usize| mb.clamp(1, 256) * 1024 * 1024)
        .unwrap_or(4 * 1024 * 1024)
}

/// Write buffer for streaming DB → `.bin` checkpoint (MiB). Default 4; clamp 1–256.
fn checkpoint_write_buf_capacity() -> usize {
    std::env::var("DISK_UTXO_CHECKPOINT_WRITE_BUF_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(|mb: usize| mb.clamp(1, 256) * 1024 * 1024)
        .unwrap_or(4 * 1024 * 1024)
}

/// Max overlay entries before spilling to RocksDB (default 8M; typical sizes ~1–2 GiB RSS).
/// Override: `DISK_UTXO_OVERLAY_CAP`.
fn overlay_cap() -> usize {
    std::env::var("DISK_UTXO_OVERLAY_CAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8_000_000)
}

/// Blocks between non-durable flushes.  Override: `DISK_UTXO_COMMIT_INTERVAL`.
fn commit_interval() -> u64 {
    std::env::var("DISK_UTXO_COMMIT_INTERVAL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000)
}

/// Cap background compaction write I/O to this many MB/s (default 0 = unlimited).
/// Set DISK_UTXO_COMPACTION_RATE_LIMIT_MB to a positive value (e.g. 300) only if BPS degrades
/// significantly as compaction accumulates, and only on drives where compaction saturates
/// foreground reads. Unlimited by default because the rehydration of 111M entries involves
/// ~100 GB of write amplification; capping it throttles startup by 10+ minutes.
/// Override: `DISK_UTXO_COMPACTION_RATE_LIMIT_MB`.
fn compaction_rate_limit_bytes_per_sec() -> u64 {
    let mb = std::env::var("DISK_UTXO_COMPACTION_RATE_LIMIT_MB")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    mb * 1024 * 1024
}

// ── key/value codec ──────────────────────────────────────────────────────────

fn outpoint_key(op: &OutPoint) -> [u8; 36] {
    let mut k = [0u8; 36];
    k[..32].copy_from_slice(&op.hash);
    k[32..36].copy_from_slice(&op.index.to_le_bytes());
    k
}

fn outpoint_from_key(k: &[u8]) -> OutPoint {
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&k[..32]);
    let index = u32::from_le_bytes(k[32..36].try_into().unwrap());
    OutPoint { hash, index }
}

fn utxo_to_bytes_into(u: &UTXO, v: &mut Vec<u8>) {
    let script: &[u8] = u.script_pubkey.as_ref();
    v.clear();
    v.reserve(8 + 8 + 1 + 4 + script.len());
    v.extend_from_slice(&u.value.to_le_bytes());
    v.extend_from_slice(&u.height.to_le_bytes());
    v.push(u.is_coinbase as u8);
    v.extend_from_slice(&(script.len() as u32).to_le_bytes());
    v.extend_from_slice(script);
}

fn utxo_from_bytes(data: &[u8]) -> Result<UTXO> {
    if data.len() < 21 {
        bail!("utxo value too short: {} bytes", data.len());
    }
    let value = i64::from_le_bytes(data[..8].try_into().unwrap());
    let height = u64::from_le_bytes(data[8..16].try_into().unwrap());
    let is_coinbase = data[16] != 0;
    let slen = u32::from_le_bytes(data[17..21].try_into().unwrap()) as usize;
    if data.len() < 21 + slen {
        bail!(
            "utxo value truncated: expected {} script bytes, got {}",
            slen,
            data.len() - 21
        );
    }
    Ok(UTXO {
        value,
        script_pubkey: data[21..21 + slen].to_vec().into(),
        height,
        is_coinbase,
    })
}

// ── overlay entry ────────────────────────────────────────────────────────────

/// `Some(utxo)` = live entry (insert/update).  `None` = tombstone (deleted from DB).
type OverlayEntry = Option<Arc<UTXO>>;

// ── helpers ─────────────────────────────────────────────────────────────────

fn open_db_options(cache_bytes: usize) -> Options {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.set_use_fsync(false);
    // Background compaction: default 4 caps RSS during L2/L3 merges (~130 MB temp per job).
    // Override with DISK_UTXO_ROCKSDB_MAX_BACKGROUND_JOBS (2–16) when the host has RAM and L0
    // backlog is starving throughput at 500k+ UTXO height.
    opts.set_max_background_jobs(rocksdb_max_background_jobs());
    opts.set_bytes_per_sync(8 * 1024 * 1024);

    let wbuf = rocksdb_write_buffer_bytes();
    // Write-path tuning: each overlay flush is millions of entries. Larger memtable (env) ⇒
    // fewer memtable→L0 cycles per flush; costs more write-buffer RAM.
    opts.set_write_buffer_size(wbuf);
    // Keep up to 4 memtables in memory (1 active + up to 3 immutable draining to L0).
    // Peak write-buffer RAM scales with DISK_UTXO_WRITE_BUFFER_MB (≈2× active+immutable typical).
    opts.set_max_write_buffer_number(4);
    opts.set_min_write_buffer_number_to_merge(1);
    // Don't trigger L0→L1 compaction until several files accumulate; batch them into one
    // large compaction rather than many small ones.
    opts.set_level_zero_file_num_compaction_trigger(8);
    opts.set_level_zero_slowdown_writes_trigger(32);
    opts.set_level_zero_stop_writes_trigger(64);
    // L1 target and SST file size scaled for a 3–4 GB UTXO set.
    opts.set_max_bytes_for_level_base(512 * 1024 * 1024);
    opts.set_target_file_size_base(128 * 1024 * 1024);
    // No compression: the UTXO DB is not shipped or stored long-term; raw speed wins.
    opts.set_compression_type(rocksdb::DBCompressionType::None);

    let cache = Cache::new_lru_cache(cache_bytes);
    let mut block = BlockBasedOptions::default();
    block.set_block_cache(&cache);
    block.set_cache_index_and_filter_blocks(true);
    // Keep index+filter blocks pinned for L0/L1 so hot-path random reads skip
    // unnecessary SST-index IOs even when block_cache pressure is high.
    block.set_pin_l0_filter_and_index_blocks_in_cache(true);
    // ~10 bits/key bloom filter cuts false-positive rate to ~1%, eliminating
    // most SST data-block reads for keys that don't exist in a given file.
    block.set_bloom_filter(10.0, false);
    opts.set_block_based_table_factory(&block);
    // For NVMe (random-IO SSD) disable OS read-ahead; RocksDB's own cache handles locality.
    opts.set_advise_random_on_open(true);
    // Use Direct I/O for all reads (user lookups + compaction background reads).  Combined with
    // use_direct_io_for_flush_and_compaction, ALL RocksDB file I/O bypasses the OS page cache so
    // Core's LevelDB chainstate gets exclusive use of the ~10 GB available page cache.  This
    // eliminates t_core_wait_ms spikes at 500k+ height where the chainstate (~8 GB) would
    // otherwise compete with RocksDB for page cache on a 16 GB machine.
    // Disable with DISK_UTXO_DIRECT_READS=0 on machines with >32 GB RAM where both DBs fit.
    opts.set_use_direct_reads(rocksdb_use_direct_reads());
    // Bypass page cache for flush/compaction I/O.  Compaction reads+writes multi-GB SST files;
    // without O_DIRECT those pages pile up in the OS page cache at ~2 MiB/block, exhausting
    // MemAvailable after only ~4000 blocks and forcing premature checkpoints.  Normal random reads
    // still go through the block_cache (LRU, sized above), which is more efficient anyway.
    opts.set_use_direct_io_for_flush_and_compaction(true);
    // With DIO, the default per-read unit is 4 KB.  Each 128 MB SST compaction file therefore
    // requires ~32 768 individual DIO syscalls.  A 2 MB readahead unit cuts that to ~64 calls
    // (512× fewer syscalls) while keeping all I/O off the page cache.
    opts.set_compaction_readahead_size(2 * 1024 * 1024);
    // Skip bloom filters on the bottommost level (L_max).  At 77M+ entries virtually all
    // existing-key lookups end up there; the bloom filter check is pure overhead since a positive
    // hit forces a binary-search anyway.  Disabling it on L_max shrinks filter memory and removes
    // the per-read bloom probe for the hot path.
    opts.set_optimize_filters_for_hits(true);

    // Cap background compaction write throughput so it doesn't saturate NVMe bandwidth and starve
    // foreground prefetch reads.  Default 300 MB/s; set DISK_UTXO_COMPACTION_RATE_LIMIT_MB=0 to
    // remove the limit entirely.
    let rate_limit = compaction_rate_limit_bytes_per_sec();
    if rate_limit > 0 {
        // refill_period_us=100_000 (100ms), fairness=10 (standard).
        opts.set_ratelimiter(rate_limit as i64, 100_000, 10);
    }

    opts
}

fn write_options_fast() -> WriteOptions {
    let mut wo = WriteOptions::default();
    wo.disable_wal(true);
    wo.set_sync(false);
    wo
}

fn write_options_durable() -> WriteOptions {
    let mut wo = WriteOptions::default();
    wo.set_sync(true);
    wo
}

fn read_stored_len(db: &DB) -> Result<Option<u64>> {
    match db.get(META_LEN_KEY)? {
        None => Ok(None),
        Some(b) if b.len() == 8 => Ok(Some(u64::from_le_bytes(b.as_slice().try_into().unwrap()))),
        Some(_) => bail!(
            "corrupt {} key in RocksDB",
            String::from_utf8_lossy(META_LEN_KEY)
        ),
    }
}

fn read_stored_tip(db: &DB) -> Result<Option<u64>> {
    match db.get(META_TIP_KEY)? {
        None => Ok(None),
        Some(b) if b.len() == 8 => Ok(Some(u64::from_le_bytes(b.as_slice().try_into().unwrap()))),
        Some(_) => bail!(
            "corrupt {} key in RocksDB",
            String::from_utf8_lossy(META_TIP_KEY)
        ),
    }
}

fn db_has_utxo_rows(db: &DB) -> Result<bool> {
    let it = db.iterator(IteratorMode::Start);
    for res in it {
        let (k, _) = res?;
        if !is_disk_utxo_meta_key(k.as_ref()) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn resolve_open_len(path: &Path, db: &DB) -> Result<u64> {
    match read_stored_len(db)? {
        Some(n) => Ok(n),
        None => {
            if db_has_utxo_rows(db)? {
                bail!(
                    "disk-utxo RocksDB at {} has UTXO rows but no length metadata; delete the directory and rehydrate",
                    path.display()
                );
            }
            Ok(0)
        }
    }
}

fn db_dir_disk_usage(path: &Path) -> u64 {
    let mut sum = 0u64;
    if let Ok(rd) = fs::read_dir(path) {
        for ent in rd.flatten() {
            let p = ent.path();
            if let Ok(m) = fs::metadata(&p) {
                if m.is_file() {
                    sum += m.len();
                }
            }
        }
    }
    sum
}

// ── DiskUtxoSet ──────────────────────────────────────────────────────────────

/// Entries being written to RocksDB in the background. Prefetch must check this for
/// keys not in the current overlay so reads stay consistent during async flushes.
type PendingFlush = Arc<Mutex<Option<Arc<HashMap<OutPoint, OverlayEntry>>>>>;

pub struct DiskUtxoSet {
    db: Arc<DB>,
    db_path: PathBuf,
    /// Last block height **durably on disk** (written in the same WriteBatch as overlay flush).
    /// On crash, RocksDB UTXO data is consistent through exactly this height.
    utxo_tip_height: Option<u64>,
    /// In-memory tip: the height of the most recent `apply_block_delta`. May be ahead of
    /// `utxo_tip_height` when overlay entries haven't been flushed yet.
    inflight_tip: Option<u64>,
    /// Authoritative count of live UTXOs (overlay + DB, deduplicated).
    len: u64,
    /// Write-back cache.  `Some(arc)` = live, `None` = tombstone pending delete in DB.
    overlay: HashMap<OutPoint, OverlayEntry>,
    /// Blocks since last RocksDB flush.
    blocks_since_commit: u64,
    commit_interval: u64,
    overlay_cap: usize,
    /// In-flight background flush (if any). Prefetch checks this for reads.
    pending_flush: PendingFlush,
    /// Handle to the background writer thread (joined on drop / checkpoint save).
    flush_thread: Option<std::thread::JoinHandle<Result<()>>>,
    /// Background checkpoint save thread. While active, overlay flushes to RocksDB are
    /// suppressed so the DB state stays frozen at the checkpoint height (guaranteeing a
    /// consistent iterator read without needing a RocksDB snapshot).
    bg_checkpoint: Option<std::thread::JoinHandle<Result<()>>>,
}

impl DiskUtxoSet {
    pub fn open(path: &Path) -> Result<Self> {
        if path.exists() && path.is_file() {
            bail!(
                "{} is a file; RocksDB needs a directory. Remove the old redb file if present or set DISK_UTXO_DB_PATH to a new directory (e.g. …/utxo_rocksdb).",
                path.display()
            );
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir for {}", path.display()))?;
        }
        let cache = rocksdb_cache_bytes();
        let bg_jobs = rocksdb_max_background_jobs();
        let wbuf_mb = rocksdb_write_buffer_bytes() / (1024 * 1024);
        let rate_limit_mb = compaction_rate_limit_bytes_per_sec() / (1024 * 1024);
        let rate_str = if rate_limit_mb == 0 {
            "unlimited".to_string()
        } else {
            format!("{rate_limit_mb} MB/s")
        };
        let direct_reads = rocksdb_use_direct_reads();
        eprintln!(
            "   [disk-utxo] opening RocksDB at {} (cache {} MiB, bg_jobs={bg_jobs}, write_buffer={wbuf_mb} MiB, compaction_rate={rate_str}, direct_reads={direct_reads})…",
            path.display(),
            cache / (1024 * 1024),
        );
        let opts = open_db_options(cache);
        eprintln!("   [disk-utxo] options built, calling DB::open…");
        let db = Arc::new(
            DB::open(&opts, path)
                .map_err(|e| anyhow::anyhow!("open RocksDB {}: {e}", path.display()))?,
        );
        eprintln!("   [disk-utxo] DB::open returned ok");
        let len = resolve_open_len(path, &db)?;
        let utxo_tip_height = read_stored_tip(&db)?;

        let cap = overlay_cap();
        let ci = commit_interval();
        eprintln!(
            "   [disk-utxo] opened {} ({len} entries, block cache {} MiB, overlay cap {cap}, commit every {ci} blocks)",
            path.display(),
            cache / (1024 * 1024),
        );
        Ok(Self {
            db,
            db_path: path.to_path_buf(),
            inflight_tip: utxo_tip_height,
            utxo_tip_height,
            len,
            overlay: HashMap::with_capacity(cap.min(1_000_000)),
            blocks_since_commit: 0,
            commit_interval: ci,
            overlay_cap: cap,
            pending_flush: Arc::new(Mutex::new(None)),
            flush_thread: None,
            bg_checkpoint: None,
        })
    }

    pub fn default_db_path() -> PathBuf {
        if let Ok(p) = std::env::var("DISK_UTXO_DB_PATH") {
            return PathBuf::from(p);
        }
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("blvm-bench")
            .join("utxo_rocksdb")
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    /// Last durable tip: the height whose UTXO data is fully on disk in RocksDB.
    pub fn utxo_tip_height(&self) -> Option<u64> {
        self.utxo_tip_height
    }

    /// In-memory tip (may be ahead of durable tip by up to one overlay cycle).
    pub fn inflight_tip(&self) -> Option<u64> {
        self.inflight_tip
    }

    /// Record that block at `height` was applied into the overlay. The tip is NOT durable
    /// until the next `flush_overlay` writes it atomically with the UTXO data.
    pub fn set_inflight_tip(&mut self, height: u64) {
        self.inflight_tip = Some(height);
    }

    /// Force-persist tip to RocksDB outside the normal flush cycle (used during rehydration
    /// and migration). Prefer `set_inflight_tip` + `flush_overlay` for normal operation.
    pub fn persist_utxo_tip_height(&mut self, height: u64) -> Result<()> {
        let mut batch = WriteBatch::default();
        batch.put(META_TIP_KEY, height.to_le_bytes());
        self.db.write_opt(batch, &write_options_fast())?;
        self.utxo_tip_height = Some(height);
        self.inflight_tip = Some(height);
        Ok(())
    }

    pub fn overlay_len(&self) -> usize {
        self.overlay.len()
    }

    // ── checkpoint load ─────────────────────────────────────────────────────

    /// Stream fixed-v1 checkpoint into RocksDB. If **`progress`** is set, writes atomic status
    /// for external monitors (e.g. `kernel-diff-status.sh`).
    pub fn load_checkpoint_fixed_v1(
        &mut self,
        checkpoint_path: &Path,
        progress: Option<&Path>,
        checkpoint_height: u64,
    ) -> Result<u64> {
        let read_cap = rehydrate_read_buf_capacity();
        let file = std::fs::File::open(checkpoint_path)
            .with_context(|| format!("open {}", checkpoint_path.display()))?;

        // Advise the kernel: sequential read, drop pages after we consume them so the
        // 8 GB checkpoint file doesn't fill page-cache and compete with RocksDB for RAM.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::io::AsRawFd;
            extern "C" {
                fn posix_fadvise(fd: i32, offset: i64, len: i64, advice: i32) -> i32;
            }
            const POSIX_FADV_SEQUENTIAL: i32 = 2;
            const POSIX_FADV_DONTNEED: i32 = 4;
            unsafe {
                posix_fadvise(file.as_raw_fd(), 0, 0, POSIX_FADV_SEQUENTIAL);
                posix_fadvise(file.as_raw_fd(), 0, 0, POSIX_FADV_DONTNEED);
            }
        }

        let mut r = std::io::BufReader::with_capacity(read_cap, file);

        let mut header = [0u8; crate::utxo_snapshot_fixed_v1::HEADER_LEN];
        r.read_exact(&mut header)?;
        if header[..8] != crate::utxo_snapshot_fixed_v1::FIXED_V1_MAGIC[..] {
            bail!("not a fixed-v1 checkpoint");
        }
        let count = u64::from_le_bytes(header[20..28].try_into().unwrap());

        let batch_rows = rehydrate_batch_rows();
        let single_tx = rehydrate_single_tx();
        eprintln!(
            "   [disk-utxo] loading {} entries from {} (read buf {} MiB, batch={batch_rows}{})",
            count,
            checkpoint_path.display(),
            read_cap / (1024 * 1024),
            if single_tx { ", SINGLE_BATCH" } else { "" },
        );
        if single_tx {
            eprintln!("   [disk-utxo] warning: SINGLE_BATCH can use many GB RAM; OOM possible on small hosts");
        }

        if self.len > 0 || db_has_utxo_rows(&self.db)? {
            bail!("disk-utxo DB is not empty; delete the DB directory first for a clean load");
        }

        let stat_every = rehydrate_status_every_entries();
        if let Some(p) = progress {
            write_rehydrate_progress(p, 0, count, checkpoint_height);
        }

        let wo_fast = write_options_fast();
        let mut loaded: u64 = 0;
        let mut next_status_at: u64 = stat_every;
        let mut vbuf = Vec::with_capacity(128);
        let overall = Instant::now();

        let bump_progress = |loaded: u64,
                             count: u64,
                             next_status_at: &mut u64,
                             progress: Option<&Path>,
                             checkpoint_height: u64| {
            if let Some(p) = progress {
                if loaded >= *next_status_at || loaded == count {
                    write_rehydrate_progress(p, loaded, count, checkpoint_height);
                    *next_status_at = loaded + stat_every;
                }
            }
        };

        if single_tx {
            let mut batch = WriteBatch::default();
            while let Some((op, utxo)) = try_read_one_fixed_v1_entry(&mut r)? {
                let k = outpoint_key(&op);
                utxo_to_bytes_into(&utxo, &mut vbuf);
                batch.put(k.as_slice(), vbuf.as_slice());
                loaded += 1;
                bump_progress(
                    loaded,
                    count,
                    &mut next_status_at,
                    progress,
                    checkpoint_height,
                );
            }
            batch.put(META_LEN_KEY, loaded.to_le_bytes());
            self.db.write_opt(batch, &wo_fast)?;
            let sec = overall.elapsed().as_secs_f64().max(1e-9);
            eprintln!(
                "   [disk-utxo] single-batch import done {loaded} rows in {sec:.1}s ({:.0} k rows/s)",
                (loaded as f64 / sec) / 1000.0
            );
        } else {
            // Read until EOF so all physical entries are loaded even when the header `count`
            // was under-recorded (e.g. due to the in-block UTXO len-tracking bug).
            'outer: loop {
                let batch_start = Instant::now();
                let at_batch_start = loaded;
                let mut batch = WriteBatch::default();
                let mut batch_len = 0u64;
                while batch_len < batch_rows {
                    match try_read_one_fixed_v1_entry(&mut r)? {
                        Some((op, utxo)) => {
                            let k = outpoint_key(&op);
                            utxo_to_bytes_into(&utxo, &mut vbuf);
                            batch.put(k.as_slice(), vbuf.as_slice());
                            loaded += 1;
                            batch_len += 1;
                            bump_progress(
                                loaded,
                                count,
                                &mut next_status_at,
                                progress,
                                checkpoint_height,
                            );
                        }
                        None => {
                            if batch_len > 0 {
                                batch.put(META_LEN_KEY, loaded.to_le_bytes());
                                self.db.write_opt(batch, &wo_fast)?;
                                let n = loaded - at_batch_start;
                                let bsec = batch_start.elapsed().as_secs_f64().max(1e-9);
                                eprintln!(
                                    "   [disk-utxo] commit (final)  +{n} rows in {bsec:.1}s ({:.0} k/s)  total {loaded}",
                                    (n as f64 / bsec) / 1000.0,
                                );
                            }
                            break 'outer;
                        }
                    }
                }
                if batch_len == 0 {
                    break;
                }
                batch.put(META_LEN_KEY, loaded.to_le_bytes());
                self.db.write_opt(batch, &wo_fast)?;

                // Release already-consumed checkpoint file pages from page-cache to
                // keep our RSS low while RocksDB compaction runs in the background.
                #[cfg(target_os = "linux")]
                {
                    use std::io::Seek;
                    if let Ok(pos) = r.seek(std::io::SeekFrom::Current(0)) {
                        extern "C" {
                            fn posix_fadvise(fd: i32, offset: i64, len: i64, advice: i32) -> i32;
                        }
                        const POSIX_FADV_DONTNEED: i32 = 4;
                        use std::os::unix::io::AsRawFd;
                        unsafe {
                            posix_fadvise(
                                r.get_ref().as_raw_fd(),
                                0,
                                pos as i64,
                                POSIX_FADV_DONTNEED,
                            );
                        }
                    }
                }

                let n = loaded - at_batch_start;
                let bsec = batch_start.elapsed().as_secs_f64().max(1e-9);
                let pct = (100.0 * loaded as f64 / count.max(loaded) as f64).min(100.0);
                eprintln!(
                    "   [disk-utxo] commit {pct:.0}%  +{n} rows in {bsec:.1}s ({:.0} k/s)  total {loaded}/{}",
                    (n as f64 / bsec) / 1000.0,
                    count,
                );
            }
            let sec = overall.elapsed().as_secs_f64().max(1e-9);
            eprintln!(
                "   [disk-utxo] import done in {sec:.1}s ({:.0} k rows/s avg) — loaded {loaded} entries (header said {count})",
                (loaded as f64 / sec) / 1000.0,
            );
        }

        let wo_dur = write_options_durable();
        let mut sync_meta = WriteBatch::default();
        sync_meta.put(META_LEN_KEY, loaded.to_le_bytes());
        sync_meta.put(META_TIP_KEY, checkpoint_height.to_le_bytes());
        self.db.write_opt(sync_meta, &wo_dur)?;

        if let Some(p) = progress {
            clear_rehydrate_progress(p);
        }

        self.len = loaded;
        self.utxo_tip_height = Some(checkpoint_height);
        Ok(count)
    }

    // ── per-block ────────────────────────────────────────────────────────────

    /// Build the in-memory `UtxoSet` that `connect_block_ibd` needs for one block.
    pub fn prefetch(&mut self, outpoints: &[OutPoint]) -> Result<UtxoSet> {
        let mut set = utxo_set_with_capacity(outpoints.len());
        let mut need_pending: Vec<OutPoint> = Vec::new();

        for op in outpoints {
            match self.overlay.get(op) {
                Some(Some(arc)) => {
                    set.insert(*op, Arc::clone(arc));
                }
                Some(None) => {} // tombstone
                None => {
                    need_pending.push(*op);
                }
            }
        }

        // Check in-flight flush for keys not in the current overlay.
        let mut need_db: Vec<OutPoint> = Vec::new();
        if !need_pending.is_empty() {
            let pending = self.pending_flush.lock().unwrap();
            if let Some(ref pf) = *pending {
                for op in need_pending {
                    match pf.get(&op) {
                        Some(Some(arc)) => {
                            set.insert(op, Arc::clone(arc));
                        }
                        Some(None) => {} // tombstone in flight
                        None => {
                            need_db.push(op);
                        }
                    }
                }
            } else {
                need_db = need_pending;
            }
        }

        if !need_db.is_empty() {
            need_db.sort_unstable_by(|a, b| a.hash.cmp(&b.hash).then(a.index.cmp(&b.index)));
            let keys: Vec<[u8; 36]> = need_db.iter().map(outpoint_key).collect();
            let key_refs: Vec<&[u8]> = keys.iter().map(|k| k.as_slice()).collect();
            let values = self.db.multi_get(key_refs);
            for (op, val_opt) in need_db.iter().zip(values) {
                let v = val_opt?;
                if let Some(data) = v {
                    let utxo = utxo_from_bytes(data.as_ref())?;
                    set.insert(*op, Arc::new(utxo));
                }
            }
        }

        Ok(set)
    }

    pub fn apply_block_delta(
        &mut self,
        pre_outpoints: &[OutPoint],
        post_set: &UtxoSet,
        prefetched: &UtxoSet,
    ) -> Result<()> {
        for op in pre_outpoints {
            if !post_set.contains_key(op) {
                // Determine if this UTXO actually exists in the persistent layer.
                // "In-block" UTXOs (created AND spent within the same block) are never
                // written to the overlay or DB — they only exist transiently in the
                // connect_block UTXO set.  We must NOT decrement `len` for them.
                let was_live = match self.overlay.insert(*op, None) {
                    Some(Some(_)) => true, // was live in overlay → real UTXO
                    Some(None) => false,   // already tombstoned → don't double-count
                    None => {
                        // Not in current overlay. Check pending_flush (flushed-but-pending),
                        // then fall back to prefetched (came from DB).
                        // In-block UTXOs are absent from all three sources.
                        let in_pending_live = self
                            .pending_flush
                            .lock()
                            .unwrap()
                            .as_ref()
                            .and_then(|p| p.get(op))
                            .map(|e| e.is_some())
                            .unwrap_or(false);
                        in_pending_live || prefetched.contains_key(op)
                    }
                };
                if was_live {
                    self.len = self.len.saturating_sub(1);
                }
            }
        }

        let pre_set: HashSet<&OutPoint> = pre_outpoints.iter().collect();
        for (op, utxo) in post_set.iter() {
            if pre_set.contains(op) {
                continue;
            }
            self.overlay.insert(*op, Some(Arc::clone(utxo)));
            self.len += 1;
        }

        self.blocks_since_commit += 1;

        let cap_exceeded = self.overlay.len() > self.overlay_cap;
        if cap_exceeded || self.blocks_since_commit >= self.commit_interval {
            // While a background checkpoint is streaming RocksDB, suppress overlay flushes so
            // the DB state stays frozen at the checkpoint height (correct iterator without a
            // snapshot). The overlay grows at most ~overlay_cap worth of extra entries during
            // the checkpoint write (~50–100s), which is bounded and tolerable.
            if !self.bg_checkpoint_active() {
                // If a previous flush is still running and we haven't hit the hard cap,
                // skip this interval-triggered flush rather than blocking on join_pending_flush.
                // The overlay keeps growing until either the flush completes (caught on the
                // next interval check) or the cap is hit (forced block).
                if cap_exceeded || !self.pending_flush_active() {
                    self.flush_overlay(false)?;
                }
            }
        }

        Ok(())
    }

    /// Wait for any in-flight background flush to complete, propagating errors.
    fn join_pending_flush(&mut self) -> Result<()> {
        if let Some(handle) = self.flush_thread.take() {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("flush thread panicked"))??;
            *self.pending_flush.lock().unwrap() = None;
            // Force mimalloc to decommit pages freed by the completed flush.
            // Without this, mimalloc holds onto the freed HashMap pages indefinitely,
            // causing RSS to grow linearly as each flush cycle frees a batch larger than
            // the previous one. mi_collect(true) returns all free pages to the OS.
            #[cfg(feature = "low-mem-alloc")]
            unsafe {
                libmimalloc_sys::mi_collect(true);
            }
        }
        Ok(())
    }

    /// Wait for any in-flight background checkpoint save to complete.
    pub fn join_bg_checkpoint(&mut self) -> Result<()> {
        if let Some(handle) = self.bg_checkpoint.take() {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("bg checkpoint thread panicked"))??;
        }
        Ok(())
    }

    /// Returns true if a background checkpoint save is currently running.
    /// While true, overlay flushes to RocksDB are suppressed so the DB state remains
    /// frozen at the checkpoint height (consistent iterator without a RocksDB snapshot).
    fn bg_checkpoint_active(&self) -> bool {
        self.bg_checkpoint
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
    }

    /// Returns true if a background overlay flush is currently in-flight.
    fn pending_flush_active(&self) -> bool {
        self.flush_thread
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
    }

    pub fn flush_overlay(&mut self, durable: bool) -> Result<()> {
        if self.overlay.is_empty() {
            return Ok(());
        }

        // Wait for any previous background flush before starting a new one.
        self.join_pending_flush()?;

        // Take the overlay, replacing it with an empty HashMap (zero capacity).
        // std::mem::take releases the old HashMap's allocated capacity when entries is dropped,
        // avoiding the retain-on-drain behavior of HashMap::drain() which keeps capacity alive.
        let entries = std::mem::take(&mut self.overlay);
        let n_entries = entries.len();
        let len_snapshot = self.len;
        let tip_snapshot = self.inflight_tip;
        self.blocks_since_commit = 0;

        // Log overlay flush size so we can track memory growth source.
        if let Some(h) = tip_snapshot {
            eprintln!(
                "   [disk-utxo] flush overlay: {} entries at height {} (rocksdb total {})",
                n_entries, h, len_snapshot
            );
        }

        if durable {
            Self::write_flush_batch(&self.db, &entries, len_snapshot, tip_snapshot, true)?;
            self.utxo_tip_height = tip_snapshot;
            // entries drops here; immediately decommit freed pages in durable-flush path.
            drop(entries);
            #[cfg(feature = "low-mem-alloc")]
            unsafe {
                libmimalloc_sys::mi_collect(true);
            }
        } else {
            let shared = Arc::new(entries);
            *self.pending_flush.lock().unwrap() = Some(Arc::clone(&shared));
            let db = Arc::clone(&self.db);
            let pf = Arc::clone(&self.pending_flush);
            let tip_for_thread = tip_snapshot;
            self.flush_thread = Some(std::thread::spawn(move || {
                let result =
                    Self::write_flush_batch(&db, &shared, len_snapshot, tip_for_thread, false);
                *pf.lock().unwrap() = None;
                result
            }));
            // Optimistic: mark durable tip after spawning (the batch will land before
            // the next flush since join_pending_flush gates it). On crash the actual
            // RocksDB may be slightly behind, but the metadata and data are in the
            // same WriteBatch so they stay consistent.
            self.utxo_tip_height = tip_snapshot;
        }
        Ok(())
    }

    fn write_flush_batch(
        db: &DB,
        entries: &HashMap<OutPoint, OverlayEntry>,
        len: u64,
        tip: Option<u64>,
        durable: bool,
    ) -> Result<()> {
        // Write in chunks to cap peak WriteBatch memory. Larger chunks ⇒ fewer DB::write calls
        // per overlay flush (env DISK_UTXO_OVERLAY_FLUSH_CHUNK_ENTRIES).
        let chunk_size = overlay_flush_chunk_entries();
        let wo_fast = write_options_fast();

        let total = entries.len();
        let mut vbuf = Vec::with_capacity(128);
        let mut batch = WriteBatch::default();
        let mut batch_count = 0usize;
        let mut done = 0usize;

        for (op, entry) in entries {
            let k = outpoint_key(op);
            match entry {
                Some(utxo) => {
                    utxo_to_bytes_into(utxo.as_ref(), &mut vbuf);
                    batch.put(k.as_slice(), vbuf.as_slice());
                }
                None => {
                    batch.delete(k.as_slice());
                }
            }
            batch_count += 1;
            done += 1;
            let is_last = done == total;
            if batch_count >= chunk_size || is_last {
                if is_last {
                    batch.put(META_LEN_KEY, len.to_le_bytes());
                    if let Some(h) = tip {
                        batch.put(META_TIP_KEY, h.to_le_bytes());
                    }
                    // Write the final data+metadata batch with WAL enabled.
                    // All preceding UTXO batches disable WAL (fast), but the metadata batch
                    // (META_TIP_KEY / META_LEN_KEY, ~16 bytes) must survive a mid-run SIGKILL.
                    // With WAL off and memtable un-flushed, a crash loses the key and the
                    // stale-DB detector on the next start incorrectly reuses dirty data.
                    // Enabling WAL only here is cheap: ~16 bytes appended to the WAL file vs
                    // ~150 MB of UTXO data that flows through WAL-less batches.  On the next
                    // open RocksDB replays the WAL and restores META_TIP_KEY atomically.
                    let mut wo_wal = WriteOptions::default();
                    wo_wal.set_sync(false); // no fsync needed, WAL survives crash already
                    if durable {
                        wo_wal.set_sync(true); // durable path: full sync
                    }
                    db.write_opt(batch, &wo_wal)?;
                } else {
                    db.write_opt(batch, &wo_fast)?;
                }
                batch = WriteBatch::default();
                batch_count = 0;
            }
        }

        // Empty overlay: still need to persist metadata.
        if total == 0 {
            let mut meta_batch = WriteBatch::default();
            meta_batch.put(META_LEN_KEY, len.to_le_bytes());
            if let Some(h) = tip {
                meta_batch.put(META_TIP_KEY, h.to_le_bytes());
            }
            let mut wo_wal = WriteOptions::default();
            wo_wal.set_sync(false);
            if durable {
                wo_wal.set_sync(true);
            }
            db.write_opt(meta_batch, &wo_wal)?;
        }

        Ok(())
    }

    // ── checkpoint save ─────────────────────────────────────────────────────

    /// Flush overlay synchronously (so RocksDB is consistent at `height`), then stream the full
    /// DB to a fixed-v1 checkpoint file **in a background thread**.  The main loop can continue
    /// processing blocks while the file is being written; overlay flushes to RocksDB are
    /// suppressed (`bg_checkpoint_active() == true`) until the thread finishes to keep the DB
    /// state frozen.
    ///
    /// Call `join_bg_checkpoint()` before starting a new checkpoint or before shutdown.
    pub fn save_checkpoint_fixed_v1(&mut self, height: u64, path: &Path) -> Result<()> {
        // Join any previous background checkpoint first.
        self.join_bg_checkpoint()?;
        self.join_pending_flush()?;
        self.flush_overlay(true)?;

        let db = Arc::clone(&self.db);
        let path = path.to_path_buf();
        let len = self.len;

        let handle = std::thread::spawn(move || {
            Self::write_checkpoint_file_from_db(&db, height, len, &path)
        });
        self.bg_checkpoint = Some(handle);
        Ok(())
    }

    fn write_checkpoint_file_from_db(db: &DB, height: u64, len: u64, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("rocksdb.part");
        let file =
            std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        let mut w = std::io::BufWriter::with_capacity(checkpoint_write_buf_capacity(), file);

        // Write header with a placeholder count; we'll rewrite it after iterating.
        // Using the actual iterator count (not `self.len`) avoids the len-undercount bug
        // where zombie entries (put-to-RocksDB but pending tombstone in overlay) inflate
        // the physical entry count beyond the tracked `len`.
        w.write_all(crate::utxo_snapshot_fixed_v1::FIXED_V1_MAGIC)?;
        w.write_all(&crate::utxo_snapshot_fixed_v1::FIXED_V1_FORMAT_VERSION.to_le_bytes())?;
        w.write_all(&height.to_le_bytes())?;
        // Placeholder for count — will be patched below.
        w.write_all(&0u64.to_le_bytes())?;

        let mut written: u64 = 0;
        let mut iter = db.iterator(IteratorMode::Start);
        while let Some(res) = iter.next() {
            let (k, v) = res?;
            if is_disk_utxo_meta_key(k.as_ref()) {
                continue;
            }
            if k.len() != 36 {
                continue;
            }
            let op = outpoint_from_key(k.as_ref());
            let utxo = utxo_from_bytes(v.as_ref())?;
            write_fixed_v1_entry_raw(&mut w, &op, &utxo)?;
            written += 1;
        }
        w.flush()?;

        // Patch the count field in the header (offset = magic(8) + version(4) + height(8) = 20).
        use std::io::{Seek, SeekFrom, Write};
        let mut inner = w.into_inner().map_err(|e| anyhow::anyhow!("flush: {e}"))?;
        inner.seek(SeekFrom::Start(20))?;
        inner.write_all(&written.to_le_bytes())?;
        drop(inner);

        if written != len {
            eprintln!(
                "   [disk-utxo] note: physical entries {written} vs tracked len {len} (len patched in header)",
            );
        }

        if path.exists() {
            let mut perms = std::fs::metadata(path)?.permissions();
            perms.set_readonly(false);
            std::fs::set_permissions(path, perms)?;
        }
        std::fs::rename(&tmp, path)?;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(path, perms)?;

        eprintln!(
            "   [disk-utxo] wrote {} (height {height}, {written} entries)",
            path.display()
        );
        Ok(())
    }

    /// Full LSM compaction (slow; optional). Used when on-disk footprint is far above expected.
    pub fn compact(&mut self, compact_status: Option<&Path>) -> Result<()> {
        self.join_bg_checkpoint()?;
        self.join_pending_flush()?;
        let before = db_dir_disk_usage(&self.db_path);
        let start_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Some(p) = compact_status {
            write_compact_status(p, before, start_ts);
        }
        eprintln!(
            "   [disk-utxo] compacting LSM (~{:.1} GiB on disk) ...",
            before as f64 / (1024.0 * 1024.0 * 1024.0),
        );
        let t = Instant::now();
        self.db
            .compact_range::<Vec<u8>, Vec<u8>>(None::<Vec<u8>>, None::<Vec<u8>>);
        let after = db_dir_disk_usage(&self.db_path);
        if let Some(p) = compact_status {
            clear_compact_status(p);
        }
        eprintln!(
            "   [disk-utxo] compact done in {:.1}s: ~{:.1} GiB → ~{:.1} GiB ({:.0}% reduction)",
            t.elapsed().as_secs_f64(),
            before as f64 / (1024.0 * 1024.0 * 1024.0),
            after as f64 / (1024.0 * 1024.0 * 1024.0),
            if before > 0 {
                100.0 * (1.0 - after as f64 / before as f64)
            } else {
                0.0
            },
        );
        Ok(())
    }

    pub fn db_file_bytes(&self) -> u64 {
        db_dir_disk_usage(&self.db_path)
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Collect all non-coinbase input OutPoints from a deserialized block.
pub fn block_input_outpoints(block: &blvm_protocol::types::Block) -> Vec<OutPoint> {
    let mut ops = Vec::new();
    for (i, tx) in block.transactions.iter().enumerate() {
        if i == 0 {
            continue;
        }
        for input in &tx.inputs {
            ops.push(input.prevout);
        }
    }
    ops
}

fn write_fixed_v1_entry_raw<W: std::io::Write>(w: &mut W, op: &OutPoint, u: &UTXO) -> Result<()> {
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

fn read_one_fixed_v1_entry<R: Read>(r: &mut R) -> Result<(OutPoint, UTXO)> {
    let mut hash = [0u8; 32];
    r.read_exact(&mut hash)?;
    let mut u4 = [0u8; 4];
    r.read_exact(&mut u4)?;
    let index = u32::from_le_bytes(u4);
    let mut u8b = [0u8; 8];
    r.read_exact(&mut u8b)?;
    let value = i64::from_le_bytes(u8b);
    r.read_exact(&mut u8b)?;
    let height = u64::from_le_bytes(u8b);
    let mut cb = [0u8; 1];
    r.read_exact(&mut cb)?;
    let is_coinbase = match cb[0] {
        0 => false,
        1 => true,
        b => bail!("fixed v1: invalid is_coinbase byte {b}"),
    };
    r.read_exact(&mut u4)?;
    let slen = u32::from_le_bytes(u4) as usize;
    let mut script = vec![0u8; slen];
    r.read_exact(&mut script)?;
    let op = OutPoint { hash, index };
    let utxo = UTXO {
        value,
        script_pubkey: script.into(),
        height,
        is_coinbase,
    };
    Ok((op, utxo))
}

/// Like `read_one_fixed_v1_entry` but returns `Ok(None)` on clean EOF (instead of error).
/// Used by `load_checkpoint_fixed_v1` to read past the header's `count` field so that
/// all physical entries are loaded even when the count was mis-recorded.
fn try_read_one_fixed_v1_entry<R: Read>(r: &mut R) -> Result<Option<(OutPoint, UTXO)>> {
    let mut hash = [0u8; 32];
    match r.read(&mut hash[..1]) {
        Ok(0) => return Ok(None), // clean EOF
        Ok(_) => {
            r.read_exact(&mut hash[1..])?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let mut u4 = [0u8; 4];
    r.read_exact(&mut u4)?;
    let index = u32::from_le_bytes(u4);
    let mut u8b = [0u8; 8];
    r.read_exact(&mut u8b)?;
    let value = i64::from_le_bytes(u8b);
    r.read_exact(&mut u8b)?;
    let height = u64::from_le_bytes(u8b);
    let mut cb = [0u8; 1];
    r.read_exact(&mut cb)?;
    let is_coinbase = match cb[0] {
        0 => false,
        1 => true,
        b => bail!("fixed v1: invalid is_coinbase byte {b}"),
    };
    r.read_exact(&mut u4)?;
    let slen = u32::from_le_bytes(u4) as usize;
    let mut script = vec![0u8; slen];
    r.read_exact(&mut script)?;
    let op = OutPoint { hash, index };
    let utxo = UTXO {
        value,
        script_pubkey: script.into(),
        height,
        is_coinbase,
    };
    Ok(Some((op, utxo)))
}
