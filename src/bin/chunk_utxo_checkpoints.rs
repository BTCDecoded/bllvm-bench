//! Build BLVM UTXO checkpoints from the **chunk cache only** (no `libbitcoinkernel`, no Core).
//!
//! Ensures **`chunks.index`** exists (builds from `chunk_*.bin.zst` **only when the file is missing**).
//! A full rescan that deletes the index requires **`--rebuild-index`** (expensive). Then walks blocks
//! and writes `{BLOCK_CACHE_DIR}/{--checkpoint-dir}/utxo_H.bin` (default **`differential_checkpoints_fixed_v1/`**).
//! when `H % checkpoint_every == 0` after a successful `connect_block` at height `H`.
//!
//! ```text
//! cargo build --release --features "scan,disk-utxo" --bin chunk_utxo_checkpoints
//! ./target/release/chunk_utxo_checkpoints --checkpoint-every 25000 --start 0 --end 500000 --format fixed-v1
//! Default: IBD connect path + trusted chunk index (no per-block header hash check). Use `--verify-chunk-block-hashes` to re-enable checks.
//!
//! **Assume-valid is on by default for checkpoint throughput:** if **`BLVM_ASSUME_VALID_HEIGHT`** is unset, this binary sets it to **`2000000`** before consensus config loads (script verification skipped for `height <` that value, subject to consensus gates). Set **`BLVM_ASSUME_VALID_HEIGHT=0`** to force full script verification on every block. `network_time` uses [`blvm_bench::kernel_diff_paths::network_time_for_historical_chunk_replay`] so Core’s two-week “buried” gate can be satisfied when assume-valid is enabled.
//!
//! **`CHUNK_UTXO_PROGRESS_EVERY`** (default **100**): stderr line **`[CHKPT] height H`** every H blocks for status tools; set to **0** to disable.
//!
//! **Machine-readable progress** (for `kernel-diff-status.sh` BPS): after each successfully connected
//! block, writes one line `HEIGHT UNIX_SECS` to **`{BLOCK_CACHE_DIR}/.chunk_utxo_checkpoints.status`**
//! (atomic replace). Override path with **`CHUNK_UTXO_STATUS_FILE`**.
//! ```

#[cfg(feature = "low-mem-alloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::{Context, Result, bail};
use blvm_bench::checkpoint_persistence::{CheckpointFormat, CheckpointManager};
use blvm_bench::chunk_index::{
    ensure_chunk_block_index, missing_chunk_bin_files, validate_utxo_chunk_cache_index,
};
use blvm_bench::chunked_cache::{ChunkedBlockIterator, load_chunk_metadata};
use blvm_bench::kernel_diff_paths::{
    network_time_for_historical_chunk_replay, resolve_block_cache_root, resolve_chunks_data_dir,
};
use blvm_protocol::block::connect_block_ibd;
use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
use blvm_protocol::types::{Network, ValidationResult};
use clap::Parser;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// If unset, consensus default is 0 (no assume-valid). Checkpoint builds want assume-valid on for speed.
const DEFAULT_ASSUME_VALID_HEIGHT: &str = "2000000";

fn utxo_checkpoint_status_path(cache_root: &Path) -> PathBuf {
    std::env::var_os("CHUNK_UTXO_STATUS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| cache_root.join(".chunk_utxo_checkpoints.status"))
}

/// Single line `HEIGHT UNIX_TIMESTAMP` for status scripts (BPS from real wall time + height).
/// Write status every N blocks (default 10) to avoid hammering USB HDD with seeks.
/// Override: `DISK_UTXO_STATUS_WRITE_EVERY`.
fn status_write_interval() -> u64 {
    std::env::var("DISK_UTXO_STATUS_WRITE_EVERY")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(10)
}

fn write_utxo_checkpoint_status(path: &Path, height: u64) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{height} {ts}\n");
    let tmp = path.with_extension("status.tmp");
    if fs::write(&tmp, line).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Read MemAvailable from /proc/meminfo, returning bytes.  Returns u64::MAX on any error so the
/// guard never fires when the info isn't readable.
fn read_mem_available_bytes() -> u64 {
    let Ok(data) = std::fs::read_to_string("/proc/meminfo") else {
        return u64::MAX;
    };
    for line in data.lines() {
        if line.starts_with("MemAvailable:") {
            let kb: u64 = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(u64::MAX);
            return kb.saturating_mul(1024);
        }
    }
    u64::MAX
}

/// VmRSS from `/proc/self/status` (bytes). Returns **0** on parse failure.
fn read_self_vm_rss_bytes() -> u64 {
    let Ok(data) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in data.lines() {
        if line.starts_with("VmRSS:") {
            let kb: u64 = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            return kb.saturating_mul(1024);
        }
    }
    0
}

/// **CHUNK_UTXO_MEM_GUARD_GB**: exit (after writing a guard checkpoint) when MemAvailable drops
/// below this level (decimal OK, e.g. `0.5`). **`0` disables** the MemAvailable-based exit only;
/// **`CHUNK_UTXO_MEM_GUARD_MAX_RSS_GB`** can still trigger a clean exit.
///
/// Default **0.5** GiB — at heights above ~550k the UTXO HashMap fills ~14 GiB of a 16 GiB
/// machine, leaving ~0.6–1.0 GiB free under normal (no browser) conditions.  The threshold
/// sits below that floor so the guard is silent when only the UTXO process is running, but
/// fires within at most `CHUNK_UTXO_MEM_CHECK_BLOCKS` blocks when a competing process
/// (browser, etc.) drives MemAvailable under 0.5 GiB.  The streaming unsorted encoder
/// (`CHUNK_UTXO_LOW_MEM=1`) adds ~1 MiB of extra allocation when writing the checkpoint, so
/// the write itself is safe at this level.
///
/// **CHUNK_UTXO_MEM_CHECK_BLOCKS**: how often we read `/proc/meminfo` (default **200**).
/// Low enough to react to sudden browser spikes within ~30–60 seconds at typical BPS.
///
/// **CHUNK_UTXO_MEM_GUARD_CONSECUTIVE**: required consecutive low-memory samples before exit
/// (default **1**). With a 200-block check interval one reading is sufficient; requiring
/// multiple samples only delays exit into OOM territory.
///
/// **CHUNK_UTXO_MEM_WARN_GB**: when MemAvailable is below this (default **2** GiB), the guard
/// samples **every block** until MemAvailable rises back above the warning level (hysteresis).
/// On machines with plenty of free RAM this rarely triggers and **`CHUNK_UTXO_MEM_CHECK_BLOCKS`**
/// stays the dominant interval. Set **`0`** to disable fast sampling (legacy behavior: only
/// every N blocks).
///
/// **CHUNK_UTXO_MEM_GUARD_MAX_RSS_GB**: optional process RSS ceiling (reads **`VmRSS`**). When set
/// to a positive value, exceeding it triggers the same clean checkpoint exit as low MemAvailable.
/// Default **unset** (disabled). Useful to catch unexpected allocator bloat; the steady-state UTXO
/// footprint on a ~16 GiB host is often ~14 GiB, so choose a limit above that baseline if enabled.
fn mem_guard_threshold_bytes() -> u64 {
    match std::env::var("CHUNK_UTXO_MEM_GUARD_GB") {
        Ok(s) if s.trim().is_empty() => {}
        Ok(s) => {
            if let Ok(f) = s.parse::<f64>() {
                if f <= 0.0 {
                    return 0;
                }
                return (f * 1_073_741_824.0).round() as u64;
            }
        }
        Err(_) => {}
    }
    (0.5 * 1_073_741_824.0) as u64
}

fn mem_guard_check_interval_blocks() -> u64 {
    std::env::var("CHUNK_UTXO_MEM_CHECK_BLOCKS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(20)
}

/// **`CHUNK_UTXO_MAX_ACCUM_ENTRIES`**: cap on `created_in_interval` entries in the delta
/// accumulator.  When the accumulator grows beyond this limit an early partial delta is
/// flushed and the accumulator is reset, preventing unbounded RAM growth at high-churn
/// heights (e.g. Ordinals era, 835k+).  Default 15 000 000 (≈ 555 MiB of HashSet RAM).
/// Set to 0 to disable the cap and rely solely on the RSS / MemAvailable guards.
fn max_accum_entries() -> usize {
    std::env::var("CHUNK_UTXO_MAX_ACCUM_ENTRIES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(15_000_000)
}

fn mem_guard_consecutive_required() -> u32 {
    std::env::var("CHUNK_UTXO_MEM_GUARD_CONSECUTIVE")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1)
}

fn mem_warn_threshold_bytes() -> u64 {
    match std::env::var("CHUNK_UTXO_MEM_WARN_GB") {
        Ok(s) if s.trim().is_empty() => {}
        Ok(s) => {
            if let Ok(f) = s.parse::<f64>() {
                if f <= 0.0 {
                    return 0;
                }
                return (f * 1_073_741_824.0).round() as u64;
            }
        }
        Err(_) => {}
    }
    (2.0 * 1_073_741_824.0) as u64
}

fn mem_guard_rss_max_bytes() -> u64 {
    match std::env::var("CHUNK_UTXO_MEM_GUARD_MAX_RSS_GB") {
        Ok(s) if s.trim().is_empty() => {}
        Ok(s) => {
            if let Ok(f) = s.parse::<f64>() {
                if f <= 0.0 {
                    return 0;
                }
                return (f * 1_073_741_824.0).round() as u64;
            }
        }
        Err(_) => {}
    }
    // Default: 10 GiB ceiling so the guard always fires before the OOM killer on a 16 GiB machine.
    (10.0 * 1_073_741_824.0) as u64
}

/// Default: log `[CHKPT] height H` every N blocks so watchers (e.g. `kernel-diff-status.sh`) see throughput
/// between sparse `wrote … utxo_H.bin` lines. Set **`CHUNK_UTXO_PROGRESS_EVERY=0`** to disable.
fn progress_every_from_env() -> u64 {
    const DEFAULT: u64 = 100;
    match std::env::var("CHUNK_UTXO_PROGRESS_EVERY") {
        Ok(s) => match s.parse::<u64>() {
            Ok(n) => n,
            Err(_) => DEFAULT,
        },
        Err(_) => DEFAULT,
    }
}

/// Set `BLVM_ASSUME_VALID_HEIGHT` before any `get_consensus_config` access so assume-valid applies.
fn apply_default_assume_valid_for_checkpoints() -> bool {
    if std::env::var_os("BLVM_ASSUME_VALID_HEIGHT").is_none() {
        std::env::set_var("BLVM_ASSUME_VALID_HEIGHT", DEFAULT_ASSUME_VALID_HEIGHT);
        true
    } else {
        false
    }
}

#[derive(Parser, Debug)]
#[command(name = "chunk_utxo_checkpoints")]
struct Args {
    #[arg(long, env = "BLOCK_CACHE_DIR")]
    block_cache_dir: Option<PathBuf>,

    #[arg(long = "start", env = "START_HEIGHT", default_value_t = 0)]
    start: u64,

    /// End height (exclusive). Omit to stream until the chunk iterator ends.
    #[arg(long = "end", env = "END_HEIGHT")]
    end: Option<u64>,

    /// Write `utxo_H.bin` every `N` heights (after successful connect at `H`).
    #[arg(long)]
    checkpoint_every: u64,

    #[arg(long, default_value = "fixed-v1")]
    format: CheckpointFormat,

    /// Subdirectory under `BLOCK_CACHE_DIR` for `utxo_*.bin` (keeps legacy `differential_checkpoints/` bincode files separate).
    #[arg(
        long,
        default_value = "differential_checkpoints_fixed_v1",
        env = "UTXO_CHECKPOINT_DIR"
    )]
    checkpoint_dir: PathBuf,

    /// Rolling retention: keep only the N most recent `utxo_*.bin` files after each write.
    /// 0 = keep all (default). Only applies to full snapshots, not deltas.
    #[arg(long, default_value_t = 0, env = "KEEP_CHECKPOINTS")]
    keep_checkpoints: usize,

    /// Write **`delta_H.bin`** on ladder steps where a full base is not written; with default
    /// **`--base-every 1`**, every rung is a full **`utxo_H.bin`**. Early accum-cap flushes can
    /// still emit extra **`delta_*.bin`** between rungs.
    #[arg(long, default_value_t = false)]
    delta_checkpoints: bool,

    /// When `--delta-checkpoints` is set: write a full **`utxo_H.bin`** every **N** checkpoint
    /// intervals (**`1`** = every rung, e.g. 900000, 1000000). **`0`** = delta at every rung; the
    /// only full written on the ladder is **`utxo_0.bin`** when height **0** is hit (**`--start 0`**).
    /// Rehydrate from **`utxo_H.bin`** otherwise supplies the base for **`delta_*.bin`**. Default **`1`**
    /// so kernel-diff lanes get a loadable snapshot each interval without chaining deltas.
    #[arg(long, default_value_t = 1, env = "BASE_EVERY")]
    base_every: u64,

    /// Load `{cache}/{--checkpoint-dir}/utxo_H.bin` and continue; **`--start` must be `H+1`**.
    #[arg(long, env = "BLVM_CHECKPOINT_HEIGHT")]
    blvm_checkpoint_height: Option<u64>,

    /// Delete `chunks.index` and rebuild from chunk files before running (expensive).
    #[arg(long, default_value_t = false)]
    rebuild_index: bool,

    /// Minimum contiguous indexed heights from `--start` (after `chunks.index` is ready).
    #[arg(long, default_value_t = 100)]
    min_contiguous: u64,

    /// If chunk files are missing per `chunks.meta`, warn only instead of failing.
    #[arg(long, default_value_t = false)]
    allow_missing_chunk_files: bool,

    /// Skip index validation (not recommended; `ChunkedBlockIterator` may still fail).
    #[arg(long, default_value_t = false)]
    skip_validation: bool,

    /// Double-SHA256 each block header and compare to `chunks.index` (slower). Default: trust the index
    /// after validation (see `validate_utxo_chunk_cache_index` when `--skip-validation` is not set).
    #[arg(long, default_value_t = false)]
    verify_chunk_block_hashes: bool,
}

fn main() -> Result<()> {
    let applied_av_default = apply_default_assume_valid_for_checkpoints();
    let args = Args::parse();

    if applied_av_default {
        eprintln!(
            "   chunk_utxo_checkpoints: BLVM_ASSUME_VALID_HEIGHT unset → using {} (assume-valid on). Set BLVM_ASSUME_VALID_HEIGHT=0 for full script verification on every block.",
            DEFAULT_ASSUME_VALID_HEIGHT
        );
    }

    if args.checkpoint_every == 0 {
        bail!("--checkpoint-every must be > 0");
    }

    let cache_root = resolve_block_cache_root(args.block_cache_dir.clone())?;
    let chunks_dir = resolve_chunks_data_dir(&cache_root)?;

    let metadata = load_chunk_metadata(&chunks_dir)?.with_context(|| {
        format!(
            "missing or invalid chunks.meta under {}",
            chunks_dir.display()
        )
    })?;

    let missing_files = missing_chunk_bin_files(&chunks_dir, metadata.num_chunks);
    if !missing_files.is_empty() {
        eprintln!(
            "   ⚠️  Missing chunk file(s) for metadata num_chunks={}: {:?} (under {})",
            metadata.num_chunks,
            missing_files,
            chunks_dir.display()
        );
        if !args.allow_missing_chunk_files {
            bail!("cannot satisfy chunks.meta: add missing chunk files or fix num_chunks / paths");
        }
    }

    let index = ensure_chunk_block_index(&chunks_dir, args.rebuild_index)?;
    if !args.skip_validation {
        let end_cap = args
            .end
            .unwrap_or(metadata.total_blocks)
            .min(metadata.total_blocks);
        validate_utxo_chunk_cache_index(&index, args.start, end_cap, args.min_contiguous)?;
    }

    let max_blocks = args.end.map(|e| {
        if e > args.start {
            (e - args.start) as usize
        } else {
            0
        }
    });

    let checkpoint_mgr =
        CheckpointManager::with_checkpoint_subdir(&cache_root, &args.checkpoint_dir)?;

    let status_path = utxo_checkpoint_status_path(cache_root.as_path());

    // With `disk-utxo`, `--start` may exceed `H+1` when resuming a warm RocksDB past the latest
    // utxo_H.bin (validated after open against `__disk_utxo_tip__`). In-memory checkpoints still
    // require `--start == H+1`.
    #[cfg(not(feature = "disk-utxo"))]
    if let Some(h) = args.blvm_checkpoint_height {
        let expect = h + 1;
        if args.start != expect {
            bail!(
                "with --blvm-checkpoint-height {h}, --start must be {expect} (next block after checkpoint)"
            );
        }
    }

    // Seek FIRST, load checkpoint SECOND.  The iterator seek decompresses through the chunk
    // file to reach `--start`; on rotational media this takes minutes and only needs ~100 MB.
    // Loading the UTXO checkpoint adds ~11+ GiB — doing that before the seek guaranteed OOM
    // on 16 GiB hosts because the seek's page-cache pressure + RSS left no headroom.
    let mut iter = if args.verify_chunk_block_hashes {
        ChunkedBlockIterator::new(&chunks_dir, Some(args.start), max_blocks)?
    } else {
        ChunkedBlockIterator::new_trust_chunk_index(&chunks_dir, Some(args.start), max_blocks)?
    }
    .context("chunk cache missing metadata or index; cannot build iterator")?;

    #[cfg(feature = "disk-utxo")]
    {
        run_disk_utxo(args, &cache_root, &checkpoint_mgr, &status_path, iter)
    }
    #[cfg(not(feature = "disk-utxo"))]
    {
        run_in_memory(args, &cache_root, &checkpoint_mgr, &status_path, &mut iter)
    }
}

#[cfg(not(feature = "disk-utxo"))]
fn run_in_memory(
    args: Args,
    _cache_root: &Path,
    checkpoint_mgr: &CheckpointManager,
    status_path: &Path,
    iter: &mut blvm_bench::chunked_cache::ChunkedBlockIterator,
) -> Result<()> {
    let mut utxo_set = blvm_protocol::UtxoSet::default();

    if let Some(h) = args.blvm_checkpoint_height {
        utxo_set = checkpoint_mgr.load_utxo_checkpoint(h)?.with_context(|| {
            format!(
                "missing checkpoint utxo_{h}.bin under {}",
                args.checkpoint_dir.display()
            )
        })?;
        eprintln!(
            "   resumed UTXO from checkpoint height {h}; continuing from height {}",
            h + 1
        );
        write_utxo_checkpoint_status(status_path, h);
    }

    let mut height = args.start;
    let progress_every = progress_every_from_env();
    let mem_guard_threshold = mem_guard_threshold_bytes();
    let mem_check_every = mem_guard_check_interval_blocks();
    let mem_guard_need_consec = mem_guard_consecutive_required();
    let mem_warn_threshold = mem_warn_threshold_bytes();
    let mem_rss_max = mem_guard_rss_max_bytes();
    let mut mem_guard_low_streak: u32 = 0;
    let mut mem_guard_fast_checks = false;

    loop {
        if let Some(end_ex) = args.end {
            if height >= end_ex {
                break;
            }
        }

        let blocks_into_run = height.saturating_sub(args.start);
        let mem_guard_enabled = mem_guard_threshold > 0 || mem_rss_max > 0;
        let sample_mem = mem_guard_enabled
            && blocks_into_run > 0
            && (blocks_into_run == 1
                || mem_guard_fast_checks
                || blocks_into_run % mem_check_every == 0);
        if sample_mem {
            let available = read_mem_available_bytes();
            if mem_warn_threshold > 0 {
                mem_guard_fast_checks = available < mem_warn_threshold;
            } else {
                mem_guard_fast_checks = false;
            }

            if mem_rss_max > 0 {
                let rss = read_self_vm_rss_bytes();
                if rss > 0 && rss > mem_rss_max {
                    let guard_height = height.saturating_sub(1);
                    eprintln!(
                        "   ⚠️  Memory guard: VmRSS={:.2}GiB > {:.2}GiB limit at height {}. \
                         Writing checkpoint and exiting cleanly.",
                        rss as f64 / 1_073_741_824.0,
                        mem_rss_max as f64 / 1_073_741_824.0,
                        guard_height
                    );
                    if guard_height > 0 {
                        checkpoint_mgr.save_utxo_checkpoint(
                            guard_height,
                            &utxo_set,
                            args.format,
                        )?;
                        eprintln!(
                            "   wrote {}/utxo_{guard_height}.bin (memory guard exit)",
                            args.checkpoint_dir.display()
                        );
                    }
                    eprintln!("   relaunch with resume-from {guard_height} to continue.");
                    let _ = std::io::stderr().flush();
                    return Ok(());
                }
            }

            if mem_guard_threshold > 0 {
                if available < mem_guard_threshold {
                    mem_guard_low_streak += 1;
                    if mem_guard_low_streak >= mem_guard_need_consec {
                        let guard_height = height.saturating_sub(1);
                        eprintln!(
                            "   ⚠️  Memory guard: MemAvailable={:.2}GiB < {:.2}GiB threshold \
                             ({}/{} consecutive checks) at height {}. Writing checkpoint and exiting cleanly.",
                            available as f64 / 1_073_741_824.0,
                            mem_guard_threshold as f64 / 1_073_741_824.0,
                            mem_guard_low_streak,
                            mem_guard_need_consec,
                            guard_height
                        );
                        if guard_height > 0 {
                            checkpoint_mgr.save_utxo_checkpoint(
                                guard_height,
                                &utxo_set,
                                args.format,
                            )?;
                            eprintln!(
                                "   wrote {}/utxo_{guard_height}.bin (memory guard exit)",
                                args.checkpoint_dir.display()
                            );
                        }
                        eprintln!("   relaunch with resume-from {guard_height} to continue.");
                        let _ = std::io::stderr().flush();
                        return Ok(());
                    }
                } else {
                    mem_guard_low_streak = 0;
                }
            }
        }

        let Some(raw) = iter.next_block()? else {
            break;
        };

        let (block, witnesses) = match deserialize_block_with_witnesses(&raw) {
            Ok(x) => x,
            Err(e) => bail!("deserialize at height {height}: {e:#}"),
        };

        let ctx = blvm_protocol::block::block_validation_context_for_connect_ibd(
            None::<&[blvm_protocol::types::BlockHeader]>,
            network_time_for_historical_chunk_replay(block.header.timestamp),
            Network::Mainnet,
        );

        let block = Arc::new(block);
        match connect_block_ibd(
            &block,
            &witnesses,
            utxo_set,
            height,
            &ctx,
            None,
            None,
            Some(Arc::clone(&block)),
            None,
        ) {
            Ok((ValidationResult::Valid, new_utxo, _, _)) => {
                utxo_set = new_utxo;
                write_utxo_checkpoint_status(status_path, height);
                if progress_every > 0 && height % progress_every == 0 {
                    eprintln!("[CHKPT] height {}", height);
                    let _ = std::io::stderr().flush();
                }
            }
            Ok((ValidationResult::Invalid(msg), _, _, _)) => {
                bail!("connect_block invalid at height {height}: {msg}");
            }
            Err(e) => bail!("connect_block error at height {height}: {e:#}"),
        }

        if height % args.checkpoint_every == 0 {
            checkpoint_mgr.save_utxo_checkpoint(height, &utxo_set, args.format)?;
            if args.keep_checkpoints > 0 {
                checkpoint_mgr.prune_old_checkpoints(args.keep_checkpoints)?;
            }
            eprintln!(
                "   wrote {}/utxo_{height}.bin ({:?})",
                args.checkpoint_dir.display(),
                args.format
            );
        }

        height = height.saturating_add(1);
    }

    eprintln!(
        "   done; last height processed = {}",
        height.saturating_sub(1)
    );
    Ok(())
}

/// Called by the memory guard when MemAvailable or RSS exceeds the limit.
/// Flushes the DB, saves a partial delta for the current in-progress interval, and
/// prints a ready-to-copy restart command so the user can continue seamlessly.
#[cfg(feature = "disk-utxo")]
fn disk_utxo_mem_guard_exit(
    height: u64,
    cp_dir: &std::path::Path,
    disk_tip_path: &std::path::Path,
    status_path: &std::path::Path,
    args: &Args,
    delta_acc: Option<&mut blvm_bench::utxo_delta::DeltaAccumulator>,
    disk_utxo: &mut blvm_bench::disk_utxo::DiskUtxoSet,
) -> Result<()> {
    // Flush any pending overlay writes to RocksDB.
    disk_utxo.join_bg_checkpoint()?;
    blvm_bench::disk_utxo::write_disk_tip_status_file(disk_tip_path, height);
    write_utxo_checkpoint_status(status_path, height);

    // Save a partial delta covering base_height..=height.
    let partial_base = if let Some(acc) = delta_acc {
        let base = acc.base_height();
        if !acc.is_empty() {
            let dp = cp_dir.join(format!("delta_{height}.bin"));
            let (na, nr) = acc.finalize_to_file(height, &dp)?;
            eprintln!(
                "   [mem-guard] wrote partial delta {height} (base={base}, added={na}, removed={nr}) → {}",
                dp.display()
            );
        } else {
            eprintln!("   [mem-guard] accumulator empty — no partial delta to write");
        }
        base
    } else {
        height
    };

    let _ = partial_base; // suppress warning if unused
    eprintln!(
        "   [mem-guard] DB tip persisted at {}. Restart with:",
        height
    );
    eprintln!(
        "     --blvm-checkpoint-height {height} --start {} --delta-checkpoints --checkpoint-every {}",
        height + 1,
        args.checkpoint_every
    );
    let _ = std::io::stderr().flush();
    Ok(())
}

/// Disk-backed UTXO path: ~1-2 GiB RSS regardless of UTXO set size. DB lives on SSD;
/// chunk reads stay on HDD. Trades some BPS for the ability to actually reach height 900k+
/// on a 16 GiB host.
#[cfg(feature = "disk-utxo")]
fn run_disk_utxo(
    args: Args,
    cache_root: &Path,
    checkpoint_mgr: &CheckpointManager,
    status_path: &Path,
    iter: blvm_bench::chunked_cache::ChunkedBlockIterator,
) -> Result<()> {
    use blvm_bench::disk_utxo::{DiskUtxoSet, block_input_outpoints};

    let db_path = DiskUtxoSet::default_db_path();
    let mut disk_utxo = DiskUtxoSet::open(&db_path)?;
    let rehydr_status_path = blvm_bench::disk_utxo::rehydrate_status_path(cache_root);
    let disk_tip_path = blvm_bench::disk_utxo::disk_tip_status_path(cache_root);

    if let Some(h) = args.blvm_checkpoint_height {
        if disk_utxo.len() == 0 {
            if args.start != h + 1 {
                bail!(
                    "`--start` must be {} when loading utxo_{}.bin into an empty RocksDB (got {})",
                    h + 1,
                    h,
                    args.start
                );
            }
            let cp_path = cache_root
                .join(&args.checkpoint_dir)
                .join(format!("utxo_{h}.bin"));
            eprintln!(
                "   [rehydrate] about to load {} into RocksDB…",
                cp_path.display()
            );
            disk_utxo.load_checkpoint_fixed_v1(&cp_path, Some(rehydr_status_path.as_path()), h)?;
            write_utxo_checkpoint_status(status_path, h);
            blvm_bench::disk_utxo::write_disk_tip_status_file(&disk_tip_path, h);
        } else {
            eprintln!(
                "   [disk-utxo] RocksDB already has {} entries; skipping utxo_{}.bin load (delete {} to force rehydrate)",
                disk_utxo.len(),
                h,
                db_path.display()
            );
            // Auto-compact if bloated: expected ~150 bytes/entry; if file is >3x that, compact.
            let force_compact =
                std::env::var("CHUNK_UTXO_FORCE_COMPACT").ok().as_deref() == Some("1");
            let file_bytes = disk_utxo.db_file_bytes();
            let expected = disk_utxo.len() as u64 * 150;
            if force_compact || file_bytes > expected * 3 {
                if force_compact {
                    eprintln!(
                        "   [disk-utxo] CHUNK_UTXO_FORCE_COMPACT=1: compacting {:.1} GiB DB with {} entries \
                         (adds bloom filters to all SST files → much faster prefetch)…",
                        file_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                        disk_utxo.len(),
                    );
                } else {
                    eprintln!(
                        "   [disk-utxo] DB file {:.1} GiB is bloated (expected ~{:.1} GiB for {} entries); compacting...",
                        file_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                        expected as f64 / (1024.0 * 1024.0 * 1024.0),
                        disk_utxo.len(),
                    );
                }
                let compact_st = blvm_bench::disk_utxo::compact_status_path(cache_root);
                disk_utxo.compact(Some(&compact_st))?;
            }
        }
    }

    if disk_utxo.len() > 0 {
        let db_tip = disk_utxo.utxo_tip_height();

        let from_file: Option<u64> = std::fs::read_to_string(&disk_tip_path)
            .ok()
            .and_then(|s| s.lines().next().map(|l| l.trim().to_owned()))
            .and_then(|t| t.parse().ok());
        let from_env: Option<u64> = std::env::var("DISK_UTXO_ASSUME_TIP_HEIGHT")
            .ok()
            .and_then(|s| s.parse().ok());

        // **Never** take `max()` across DB vs sidecar: after a crash, `.chunk_utxo_disk_tip` can
        // reflect last *connected* height while RocksDB durable tip (and UTXO rows) lag pending
        // overlay flush. Raising `META_TIP_KEY` without replaying blocks produces "UTXO not found"
        // on the next connect. When DB has a tip, it is authoritative.
        let tip: u64 = if let Some(d) = db_tip {
            if let Some(f) = from_file {
                if f > d {
                    eprintln!(
                        "   [disk-utxo] WARNING: sidecar {} claims tip {f} but RocksDB durable tip is {d}. \
Using DB tip — resume with --start {} (or delete {} to rehydrate from utxo_*.bin).",
                        disk_tip_path.display(),
                        d.saturating_add(1),
                        db_path.display(),
                    );
                }
            }
            if let Some(e) = from_env {
                if e != d {
                    eprintln!(
                        "   [disk-utxo] NOTE: DISK_UTXO_ASSUME_TIP_HEIGHT={e} ignored (RocksDB has tip {d}).",
                    );
                }
            }
            d
        } else {
            let fallback = [from_file, from_env]
                .into_iter()
                .flatten()
                .max()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "disk-utxo RocksDB at {} has data but no tip metadata in DB. \
                         Set DISK_UTXO_ASSUME_TIP_HEIGHT to the last applied block height, \
                         or populate {} (one line: height), \
                         or delete the DB and rehydrate from utxo_*.bin.",
                        db_path.display(),
                        disk_tip_path.display()
                    )
                })?;
            eprintln!(
                "   [disk-utxo] no META_TIP_KEY in DB; using tip {} from sidecar/env and persisting metadata",
                fallback
            );
            disk_utxo.persist_utxo_tip_height(fallback)?;
            fallback
        };

        if args.start != tip + 1 {
            bail!(
                "`--start` {} must equal DB UTXO tip height + 1 (DB tip is {}). Use --start {} or fix {}; delete {} to reload from .bin.",
                args.start,
                tip,
                tip + 1,
                disk_tip_path.display(),
                db_path.display()
            );
        }
        if let Some(h) = args.blvm_checkpoint_height {
            if h > tip {
                bail!(
                    "--blvm-checkpoint-height {h} is above DB UTXO tip {tip}; drop the flag or use a height ≤ tip (restart from .bin: delete {})",
                    db_path.display()
                );
            }
        }
        write_utxo_checkpoint_status(status_path, tip);
        blvm_bench::disk_utxo::write_disk_tip_status_file(&disk_tip_path, tip);
        if let Some(h) = args.blvm_checkpoint_height {
            eprintln!(
                "   [disk-utxo] UTXO tip {tip} (checkpoint ladder ref utxo_{h}.bin); connecting block {}",
                args.start
            );
        } else {
            eprintln!(
                "   [disk-utxo] UTXO tip {tip}; connecting block {}",
                args.start
            );
        }
    }

    let cp_dir = cache_root.join(&args.checkpoint_dir);

    let mut height = args.start;
    let mut delta_acc = if args.delta_checkpoints {
        let base_h = args.blvm_checkpoint_height.unwrap_or(0);
        Some(blvm_bench::utxo_delta::DeltaAccumulator::new(
            base_h, &cp_dir,
        )?)
    } else {
        None
    };
    let mut checkpoint_count: u64 = 0;

    let progress_every = progress_every_from_env();
    let mem_guard_threshold = mem_guard_threshold_bytes();
    let mem_check_every = mem_guard_check_interval_blocks();
    let accum_entry_cap = max_accum_entries();
    let mem_guard_need_consec = mem_guard_consecutive_required();
    let mem_warn_threshold = mem_warn_threshold_bytes();
    let mem_rss_max = mem_guard_rss_max_bytes();
    let mut mem_guard_low_streak: u32 = 0;
    let mut mem_guard_fast_checks = false;

    let timing_every: u64 = std::env::var("DISK_UTXO_TIMING_EVERY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);

    // How many pre-deserialized blocks to buffer in the read-ahead channel.
    // 4 is enough to keep the reader thread busy without wasting much RAM.
    let lookahead: usize = std::env::var("DISK_UTXO_LOOKAHEAD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);

    // Spawn a background thread to read and deserialize blocks ahead of the main loop.
    // This hides the deserialization CPU cost (~80-90ms/block) behind the prefetch+connect+apply
    // work in the main thread, roughly doubling throughput at steady state.
    let (block_tx, block_rx) = std::sync::mpsc::sync_channel(lookahead);
    let end_ex_rd = args.end;
    let start_rd = args.start;
    let reader_thread = std::thread::spawn(move || -> Result<()> {
        let mut iter = iter;
        let mut h = start_rd;
        loop {
            if let Some(end_ex) = end_ex_rd {
                if h >= end_ex {
                    break;
                }
            }
            let Some(raw) = iter.next_block()? else {
                break;
            };
            let parsed: Result<_> = deserialize_block_with_witnesses(&raw)
                .with_context(|| format!("deserialize at height {h}"));
            let is_err = parsed.is_err();
            if block_tx.send(parsed).is_err() {
                break;
            }
            if is_err {
                break;
            }
            h += 1;
        }
        Ok(())
    });

    let mut t_read_wait_us: u64 = 0;
    let mut t_prefetch_us: u64 = 0;
    let mut t_connect_us: u64 = 0;
    let mut t_apply_us: u64 = 0;
    let mut timing_blk: u64 = 0;
    let timing_start = Instant::now();
    let st_write_every = status_write_interval();

    // Interval timing: track per-window stats for recent BPS (not dragged by startup).
    let mut iv_read_us: u64 = 0;
    let mut iv_prefetch_us: u64 = 0;
    let mut iv_connect_us: u64 = 0;
    let mut iv_apply_us: u64 = 0;
    let mut iv_start = Instant::now();

    loop {
        let t0 = Instant::now();
        let item = match block_rx.recv() {
            Ok(r) => r,
            Err(_) => break, // read thread finished
        };
        let rw = t0.elapsed().as_micros() as u64;
        t_read_wait_us += rw;
        iv_read_us += rw;

        let (block, witnesses) = item?;

        let t1 = Instant::now();
        let input_ops = block_input_outpoints(&block);
        let prefetched = disk_utxo.prefetch(&input_ops)?;
        let pf = t1.elapsed().as_micros() as u64;
        t_prefetch_us += pf;
        iv_prefetch_us += pf;

        let t2 = Instant::now();
        let ctx = blvm_protocol::block::block_validation_context_for_connect_ibd(
            None::<&[blvm_protocol::types::BlockHeader]>,
            network_time_for_historical_chunk_replay(block.header.timestamp),
            Network::Mainnet,
        );

        let block = Arc::new(block);
        match connect_block_ibd(
            &block,
            &witnesses,
            prefetched.clone(),
            height,
            &ctx,
            None,
            None,
            Some(Arc::clone(&block)),
            None,
        ) {
            Ok((ValidationResult::Valid, post_set, _, _)) => {
                let cn = t2.elapsed().as_micros() as u64;
                t_connect_us += cn;
                iv_connect_us += cn;
                let t3 = Instant::now();
                let durable_tip_before = disk_utxo.utxo_tip_height();
                // Must not `apply_block_delta` if delta/snapshot encoding would reject an output
                // (avoids durable tip vs chain mismatch on resume after finalize/read failure).
                blvm_bench::utxo_snapshot_fixed_v1::check_utxoset_checkpointable(&post_set)?;
                // Set inflight_tip BEFORE apply_block_delta so any overlay flush that fires
                // inside (triggered by overlay_cap or commit_interval) writes tip=height
                // together with this block's data. Without this, the flush could write
                // tip=(height-1) while already containing tombstones for this block's inputs,
                // causing "UTXO not found" on the next restart at `height`.
                disk_utxo.set_inflight_tip(height);
                disk_utxo.apply_block_delta(&input_ops, &post_set, &prefetched)?;
                if let Some(acc) = delta_acc.as_mut() {
                    acc.record_block(&input_ops, &post_set)?;
                }
                let ap = t3.elapsed().as_micros() as u64;
                t_apply_us += ap;
                iv_apply_us += ap;

                // ── Accumulator size cap: early partial finalize ──────────────────────
                // At Ordinals-era heights (835k+) `created_in_interval` can reach
                // 300M+ entries per 100k blocks.  Cap it so RSS never blows up.
                if accum_entry_cap > 0 {
                    if let Some(acc) = delta_acc.as_mut() {
                        let (live_adds, _) = acc.stats();
                        if live_adds >= accum_entry_cap {
                            let dp = cp_dir.join(format!("delta_{height}.bin"));
                            let (na, nr) = acc.finalize_to_file(height, &dp)?;
                            eprintln!(
                                "   [accum-cap] height {height}: early delta (added {na}, removed {nr}, \
                                 cap={accum_entry_cap}) → {}",
                                dp.display()
                            );
                            acc.reset(height)?;
                        }
                    }
                }
                // Status file: last **connected** height — kernel-diff-status BPS needs this to advance
                // every N blocks. Durable tip (RocksDB meta) may lag by up to one overlay cycle.
                if height % st_write_every == 0 {
                    write_utxo_checkpoint_status(status_path, height);
                }
                // Sidecar: only when a flush advanced the durable tip (crash-safe resume).
                if disk_utxo.utxo_tip_height() != durable_tip_before {
                    if let Some(t) = disk_utxo.utxo_tip_height() {
                        blvm_bench::disk_utxo::write_disk_tip_status_file(&disk_tip_path, t);
                    }
                }
                timing_blk += 1;
                if progress_every > 0 && height % progress_every == 0 {
                    let (acc_adds, acc_rems) =
                        delta_acc.as_ref().map(|a| a.stats()).unwrap_or((0, 0));
                    eprintln!(
                        "[CHKPT] height {} (disk-utxo, {} entries, overlay {}, delta +{} -{})",
                        height,
                        disk_utxo.len(),
                        disk_utxo.overlay_len(),
                        acc_adds,
                        acc_rems,
                    );
                    let _ = std::io::stderr().flush();
                }

                // ── Memory guard ──────────────────────────────────────────────────────────
                let blocks_into_run = height.saturating_sub(args.start);
                let mem_guard_enabled = mem_guard_threshold > 0 || mem_rss_max > 0;
                let sample_mem = mem_guard_enabled
                    && blocks_into_run > 0
                    && (blocks_into_run == 1
                        || mem_guard_fast_checks
                        || blocks_into_run % mem_check_every == 0);
                if sample_mem {
                    let available = read_mem_available_bytes();
                    if mem_warn_threshold > 0 {
                        mem_guard_fast_checks = available < mem_warn_threshold;
                    } else {
                        mem_guard_fast_checks = false;
                    }

                    // RSS ceiling check.
                    if mem_rss_max > 0 {
                        let rss = read_self_vm_rss_bytes();
                        if rss > 0 && rss > mem_rss_max {
                            eprintln!(
                                "   ⚠️  [mem-guard] VmRSS={:.2}GiB > {:.2}GiB limit at height {}. \
                                 Saving partial delta and exiting cleanly.",
                                rss as f64 / 1_073_741_824.0,
                                mem_rss_max as f64 / 1_073_741_824.0,
                                height
                            );
                            disk_utxo_mem_guard_exit(
                                height,
                                &cp_dir,
                                &disk_tip_path,
                                status_path,
                                &args,
                                delta_acc.as_mut(),
                                &mut disk_utxo,
                            )?;
                            // Drop reader channel so the read thread exits.
                            drop(block_rx);
                            let _ = reader_thread.join();
                            return Ok(());
                        }
                    }

                    // MemAvailable check.
                    if mem_guard_threshold > 0 {
                        if available < mem_guard_threshold {
                            mem_guard_low_streak += 1;
                            if mem_guard_low_streak >= mem_guard_need_consec {
                                eprintln!(
                                    "   ⚠️  [mem-guard] MemAvailable={:.2}GiB < {:.2}GiB threshold \
                                     ({}/{} consecutive) at height {}. Saving partial delta and exiting.",
                                    available as f64 / 1_073_741_824.0,
                                    mem_guard_threshold as f64 / 1_073_741_824.0,
                                    mem_guard_low_streak,
                                    mem_guard_need_consec,
                                    height
                                );
                                disk_utxo_mem_guard_exit(
                                    height,
                                    &cp_dir,
                                    &disk_tip_path,
                                    status_path,
                                    &args,
                                    delta_acc.as_mut(),
                                    &mut disk_utxo,
                                )?;
                                drop(block_rx);
                                let _ = reader_thread.join();
                                return Ok(());
                            }
                        } else {
                            mem_guard_low_streak = 0;
                        }
                    }
                }
                // ── end memory guard ──────────────────────────────────────────────────────
            }
            Ok((ValidationResult::Invalid(msg), _, _, _)) => {
                bail!("connect_block invalid at height {height}: {msg}");
            }
            Err(e) => bail!("connect_block error at height {height}: {e:#}"),
        }

        if timing_blk > 0 && timing_blk % timing_every == 0 {
            let wall = timing_start.elapsed().as_secs_f64().max(1e-9);
            let cum_bps = timing_blk as f64 / wall;
            let iv_wall = iv_start.elapsed().as_secs_f64().max(1e-9);
            let iv_bps = timing_every as f64 / iv_wall;
            eprintln!(
                "[TIMING] {timing_blk} blks {wall:.0}s cum={cum_bps:.1} BPS  last {timing_every}={iv_bps:.1} BPS  |  interval: rw={:.0}ms pf={:.0}ms cn={:.0}ms ap={:.0}ms  overlay={}",
                iv_read_us as f64 / timing_every as f64 / 1000.0,
                iv_prefetch_us as f64 / timing_every as f64 / 1000.0,
                iv_connect_us as f64 / timing_every as f64 / 1000.0,
                iv_apply_us as f64 / timing_every as f64 / 1000.0,
                disk_utxo.overlay_len(),
            );
            let _ = std::io::stderr().flush();
            iv_read_us = 0;
            iv_prefetch_us = 0;
            iv_connect_us = 0;
            iv_apply_us = 0;
            iv_start = Instant::now();
        }

        if height % args.checkpoint_every == 0 {
            checkpoint_count += 1;

            if let Some(acc) = delta_acc.as_mut() {
                // base_every == 0: delta-only ladder except utxo_0.bin at genesis (height 0).
                // base_every >= 1: first rung full, then every Nth rung full (N==1 → every rung).
                let write_full_utxo = if args.base_every == 0 {
                    height == 0
                } else {
                    checkpoint_count == 1 || checkpoint_count % args.base_every == 0
                };

                if write_full_utxo {
                    let cp_path = cp_dir.join(format!("utxo_{height}.bin"));
                    disk_utxo.save_checkpoint_fixed_v1(height, &cp_path)?;
                    // Reset accumulator so next delta is relative to this base.
                    acc.reset(height)?;
                    eprintln!(
                        "   [checkpoint] height {height}: full base snapshot (flushed to RocksDB, writing .bin in background)"
                    );
                } else {
                    let dp = cp_dir.join(format!("delta_{height}.bin"));
                    let (na, nr) = acc.finalize_to_file(height, &dp)?;
                    eprintln!(
                        "   [checkpoint] height {height}: delta (added {na}, removed {nr}) → {}",
                        dp.display()
                    );
                }
            } else {
                let cp_path = cp_dir.join(format!("utxo_{height}.bin"));
                disk_utxo.save_checkpoint_fixed_v1(height, &cp_path)?;
                eprintln!(
                    "   [checkpoint] height {height}: flushed to RocksDB, writing .bin in background"
                );
            }

            blvm_bench::disk_utxo::write_disk_tip_status_file(&disk_tip_path, height);
            if args.keep_checkpoints > 0 {
                checkpoint_mgr.prune_old_checkpoints(args.keep_checkpoints)?;
            }
        }

        height = height.saturating_add(1);
    }

    // Collect reader thread result — propagate any error it hit.
    match reader_thread.join() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => bail!("block reader thread failed: {e:#}"),
        Err(_) => bail!("block reader thread panicked"),
    }

    let final_height = height.saturating_sub(1);

    // Flush any remaining overlay data durably so the final snapshot is complete.
    disk_utxo.flush_overlay(true)?;
    blvm_bench::disk_utxo::write_disk_tip_status_file(&disk_tip_path, final_height);

    // Write a final delta for any accumulator data not yet written (tail of the last
    // checkpoint interval or cap cycle).
    if let Some(acc) = delta_acc.as_mut() {
        if !acc.is_empty() {
            let dp = cp_dir.join(format!("delta_{final_height}.bin"));
            let (na, nr) = acc.finalize_to_file(final_height, &dp)?;
            eprintln!(
                "   [done] final delta {final_height} (added {na}, removed {nr}) → {}",
                dp.display()
            );
        }
    }

    // Always write a full tip snapshot so the caller has a loadable base at chain tip.
    disk_utxo.join_bg_checkpoint()?;
    let tip_cp = cp_dir.join(format!("utxo_{final_height}.bin"));
    disk_utxo.save_checkpoint_fixed_v1(final_height, &tip_cp)?;
    disk_utxo.join_bg_checkpoint()?;
    eprintln!(
        "   [done] tip snapshot → {} ({} entries)",
        tip_cp.display(),
        disk_utxo.len()
    );

    eprintln!(
        "   done; last height processed = {} (disk-utxo, {} entries)",
        final_height,
        disk_utxo.len()
    );
    Ok(())
}
