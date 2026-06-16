#!/usr/bin/env bash
# Self-looping runner for block_kernel_diff (default last block height 900000; KERNEL_DIFF_END=900001).
# Runs the binary in repeated chunks, auto-restarting from the last checkpoint whenever
# RSS exceeds the safety limit. No manual intervention needed to reach the target height.
#
# Logs like "Opening chunk 3" refer to the **on-disk block archive** (e.g. chunk_3.bin.zst) for
# that height range — not to this script's loop counter. Each **new process** after a resume must
# seek that file from the front; restarts are driven by MEM_AVAILABLE / RSS guards, not by chunk files.
#
# Do not confuse:
#   • KERNEL_DIFF_ASSUME — BLVM skips **script/signature** work for height < ASSUME (fast path). It does
#     **not** mean “skip re-diffing 0..N because we trust it”; that is **checkpoint resume** instead.
#   • Resume — `--blvm-checkpoint-height H` and `--start H+1` with the delta ladder / disk tip: avoids
#     **replaying** blocks 0..H; still **compares** from H+1 onward.
#
# Performance tuning (BPS) — change **one** knob at a time; compare `KERNEL_DIFF_PROGRESS` `bps=` over the
# same height window. Stops here at KERNEL_DIFF_PROGRESS_EVERY (below); resume/assume/env continues after.
#   • **Layout** — `BLOCK_CACHE_DIR`: chunk zstd + delta ladder (sequential I/O). `CORE_DATADIR` and
#     `DISK_UTXO_PATH`: NVMe preferred (random LevelDB/RocksDB I/O); avoid putting DBs on the chunk SSD.
#   • **BLVM RocksDB** — `DISK_UTXO_ROCKSDB_CACHE_BYTES`: larger → fewer random read stalls as the UTXO set
#     grows (try 1536–2048 MiB if `MemAvailable` stays above your floor).
#   • **BLVM overlay** — `DISK_UTXO_OVERLAY_CAP`: higher → more RSS, fewer RocksDB spills; lower saves RAM
#     (script default 3M). Trade with RocksDB cache using profiling, not both maxed blindly.
#   • **BLVM flush cadence** — `DISK_UTXO_COMMIT_INTERVAL`: larger batches disk work (less flush overhead)
#     but holds more in the overlay between commits (default 500).
#   • **Core coins cache** — `COINS_CACHE_MB`: raise toward 768–1024 if Core is the bottleneck and RAM fits
#     under `RSS_LIMIT_MB` / MemAvailable (script default 500).
#   • **Core workers** — `WORKER_THREADS`: script default 2 to limit RAM; try 3–4 on many-core hosts if
#     RSS and MemAvailable stay healthy (helps less when `KERNEL_SKIP_SCRIPTS=1`, but not zero).
#   • **Core chainstate in RAM** — `KERNEL_CHAINSTATE_RAM=1`: faster UTXO path, large MemEnv growth; only
#     on large-RAM machines; watch MemAvailable restarts.
#   • **Checkpoint writes** — `CHUNK_CHECKPOINT_EVERY`: larger → fewer big synchronous writes (smoother BPS),
#     coarser resume points if the process dies (default 25000).
#   • **MemAvailable** — `MEM_AVAILABLE_FLOOR_MB` / `MEM_AVAILABLE_RECOVERY_*`: every recovery pass stalls
#     throughput; tune only with spare RAM or accept OOM risk (`MEM_AVAILABLE_FLOOR_MB=0` disables guard).
#   • **Progress / watch noise** — `KERNEL_DIFF_PROGRESS_EVERY`: stderr `KERNEL_DIFF_PROGRESS` every N
#     compared blocks (script default 100; binary default 1000). **1** = every block but also runs
#     `malloc_trim` each time in the binary and **can reduce BPS**; use for debugging, not steady state.
#   • **Chunk read-ahead** — `KERNEL_DIFF_CHUNK_READAHEAD` (default 8): how many raw blocks the chunk
#     reader thread decodes ahead of the compare loop (2–64). Raise if BPS jitters during RocksDB
#     overlay commits; costs RAM for queued block buffers.
#   • **Throughput preset** — `KERNEL_DIFF_THROUGHPUT=1` (before sourcing / export): raises commit
#     interval, RocksDB cache, compaction jobs, memtable size, Core workers/coins cache, fewer
#     checkpoint writes. Needs RAM. **~10× wall-clock** needs **parallel lanes** (disjoint heights).
#   • **RocksDB env** (advanced): `DISK_UTXO_ROCKSDB_MAX_BACKGROUND_JOBS`, `DISK_UTXO_WRITE_BUFFER_MB`,
#     `DISK_UTXO_OVERLAY_FLUSH_CHUNK_ENTRIES` — see `disk_utxo.rs` module docs in blvm-bench.
#
# Env (optional):
#   KERNEL_DIFF_ENV_FILE      optional path to env file (LAN RPC); else first of:
#                             blvm-bench/kernel-diff.local.env, .env.local, .env (see .env.example)
#   BLOCK_CACHE_DIR           chunk zstd + UTXO checkpoint tree (default: ~/.local/share/blvm-kernel-diff/chunk-cache)
#   BITCOIN_CORE_LIB_DIR      required: directory containing libbitcoinkernel.a (or .so)
#   KERNEL_DIFF_NVME_ROOT     default: ~/.local/share/blvm-kernel-diff — CORE_DATADIR + DISK_UTXO_PATH
#   CORE_DATADIR              default: $KERNEL_DIFF_NVME_ROOT/core-datadir (Core LevelDB)
#   CORE_BLOCKS_DIR           default: same as CORE_DATADIR/blocks
#   DISK_UTXO_PATH            default: $KERNEL_DIFF_NVME_ROOT/disk-utxo (BLVM RocksDB)
#   DISK_UTXO_ROCKSDB_CACHE_BYTES  default: 1.5 GiB — shared page-cache model: RocksDB reads use
#                               buffered I/O (default), so block cache + page cache cooperate.
#                               1.5 GiB covers hot UTXO slots; colder data served from page cache.
#   DISK_UTXO_DIRECT_READS         default: 0 (disabled) — RocksDB reads use buffered I/O,
#                               sharing the OS page cache with Core's LevelDB.  Enabling Direct I/O
#                               (set 1) increases BLVM RSS by ~2 GB (page-cache data migrates into
#                               process heap), leaving less total cache for both DBs.  Only useful
#                               on machines with >32 GB RAM.
#   DISK_UTXO_COMMIT_INTERVAL      default: 500 (check flush every N blocks; skips if prev flush running)
#   DISK_UTXO_OVERLAY_CAP          default: 1000000 (in-memory overlay entries before spilling to
#                               RocksDB; 1M entries ≈ 120 MB RSS at ~120 bytes/entry.  Lower than
#                               the old 3-6M to reduce BLVM RSS and leave more page cache for Core.)
#   LOG_STEM                  default: kernel_diff_0_500k_v2
#   RSS_LIMIT_MB              default: 22000 (22 GiB per lane; 4×22=88 GiB on 91 GiB host).
#                               With KERNEL_BLOCKTREE_RAM=0 restarts use --import-from-core-tip
#                               → SeedHeadlessRestore (~1 min, no seek) so hitting this cap
#                               is cheap.  Set to 0 to disable and rely solely on MEM_AVAILABLE.)
#   MEM_AVAILABLE_FLOOR_MB    default: 2000 (stop when MemAvailable drops below 2 GiB; 0 = disabled).
#                               5000 was too strict on 16 GiB hosts — exits every ~1.5k blocks (~860k+)
#                               before reaching 900k.  2000 still avoids most swap thrash; use 0 only if
#                               you accept OOM risk to push one long uninterrupted run.
#   MEM_AVAILABLE_RECOVERY_PASSES  default: 3 — before exiting on MemAvailable, block_kernel_diff
#                               tries malloc_trim + durable disk-UTXO overlay flush (+ mimalloc collect)
#                               this many times; if MemAvailable recovers, continues **same process**
#                               without the wrapper restarting. Set 0 to restore immediate exit.
#   MEM_AVAILABLE_RECOVERY_SLOP_MB default: 250 — after a recovery pass, treat MemAvailable as OK
#                               if avail ≥ floor minus this (page-cache jitter).
#   WORKER_THREADS            default: 2  (Core parallel workers; low count frees CPU for main loop)
#   KERNEL_SKIP_SCRIPTS       default: 1  (tell Core's kernel to skip all script evaluation;
#                                          redundant with assume-valid but eliminates any
#                                          per-script overhead in the ConnectBlock hot path)
#   COINS_CACHE_MB            default: 1500 (Core LevelDB coins cache; balances write batching vs RAM.
#                               2000 helped t_core_wait at 640k but + strict 5 GiB MemAvailable floor
#                               caused exit every ~1.5k blocks on 16 GiB before height 900k.)
#   KERNEL_BLOCKTREE_RAM      default: 0 (write Core's block *index* to LevelDB on disk so that
#                               --import-from-core-tip works on restart; avoids repeated seed_headless
#                               fragmentation.  Set to 1 only if you never need clean restarts.)
#   CHUNK_CHECKPOINT_EVERY    default: 25000 (write checkpoint every N blocks within a run)
#   KERNEL_DIFF_PROGRESS_EVERY default: 100 — stderr KERNEL_DIFF_PROGRESS every N compared blocks
#                               (binary default is 1000; lower = more frequent height/bps in the
#                               runner log; 1 = every block but also triggers malloc_trim every
#                               block — can slow the run; see block_kernel_diff --help)
#   KERNEL_DIFF_RESUME_HEIGHT optional: if set, use this as --blvm-checkpoint-height H and
#                               START=H+1 for the *first* invocation only (before the main loop),
#                               instead of parsing RUNNER_LOG. Use after a consensus fix or when the
#                               log's last RESUME points at a height with no utxo_H.bin / broken ladder.
#   KERNEL_DIFF_END           default: 900001 — last block compared is END−1 (900000)
#   KERNEL_DIFF_ASSUME        default: 938343 — BLVM skips script/signature for height < this (mainnet
#                               assume-valid height in Core v28+). For 0..900000, 900000 < 938343 so the
#                               whole window is script-free on BLVM; pair with KERNEL_SKIP_SCRIPTS=1 on Core.
#
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Optional LAN RPC / paths: gitignored files (see .env.example). First match wins.
for _kf in "${KERNEL_DIFF_ENV_FILE:-}" \
           "$SCRIPT_ROOT/kernel-diff.local.env" \
           "$SCRIPT_ROOT/.env.local" \
           "$SCRIPT_ROOT/.env"; do
  [[ -z "${_kf:-}" ]] && continue
  [[ -f "$_kf" ]] || continue
  set -a
  # shellcheck source=/dev/null
  source "$_kf"
  set +a
  echo "Loaded kernel-diff env from $_kf" >&2
  break
done

# ── KERNEL_DIFF_THROUGHPUT=1 — preset for 500k+ height when BPS collapses (RocksDB + overlay bound). ──
# Expect materially higher BPS on 32+ GiB hosts with NVMe for DISK_UTXO_PATH and CORE_DATADIR.
# For ~10× wall-clock vs one process, run disjoint ranges: scripts/kernel-diff-parallel-lanes.sh
if [[ "${KERNEL_DIFF_THROUGHPUT:-}" == "1" ]]; then
  export DISK_UTXO_COMMIT_INTERVAL="${DISK_UTXO_COMMIT_INTERVAL:-2000}"
  export DISK_UTXO_ROCKSDB_CACHE_BYTES="${DISK_UTXO_ROCKSDB_CACHE_BYTES:-$((3*1024*1024*1024))}"
  export DISK_UTXO_ROCKSDB_MAX_BACKGROUND_JOBS="${DISK_UTXO_ROCKSDB_MAX_BACKGROUND_JOBS:-8}"
  export DISK_UTXO_WRITE_BUFFER_MB="${DISK_UTXO_WRITE_BUFFER_MB:-256}"
  export DISK_UTXO_OVERLAY_FLUSH_CHUNK_ENTRIES="${DISK_UTXO_OVERLAY_FLUSH_CHUNK_ENTRIES:-400000}"
  export DISK_UTXO_COMPACTION_RATE_LIMIT_MB="${DISK_UTXO_COMPACTION_RATE_LIMIT_MB:-200}"
  export WORKER_THREADS="${WORKER_THREADS:-4}"
  export COINS_CACHE_MB="${COINS_CACHE_MB:-750}"
  export CHUNK_CHECKPOINT_EVERY="${CHUNK_CHECKPOINT_EVERY:-50000}"
  echo "KERNEL_DIFF_THROUGHPUT=1: aggressive DISK_UTXO + Core defaults (see script header)" >&2
fi

KERNEL_DIFF_DATA_ROOT="${KERNEL_DIFF_DATA_ROOT:-${HOME}/.local/share/blvm-kernel-diff}"
BLOCK_CACHE_DIR="${BLOCK_CACHE_DIR:-$KERNEL_DIFF_DATA_ROOT/chunk-cache}"
# Databases only: Core chainstate + BLVM RocksDB (random I/O). Chunks + UTXO *.bin checkpoints
# stay under BLOCK_CACHE_DIR (SSD) via --checkpoint-dir relative to block cache.
KERNEL_DIFF_NVME_ROOT="${KERNEL_DIFF_NVME_ROOT:-$KERNEL_DIFF_DATA_ROOT}"
CORE_DATADIR="${CORE_DATADIR:-$KERNEL_DIFF_NVME_ROOT/core-datadir}"
CORE_BLOCKS_DIR="${CORE_BLOCKS_DIR:-}"
DISK_UTXO_PATH="${DISK_UTXO_PATH-$KERNEL_DIFF_NVME_ROOT/disk-utxo}"  # note: -  not :-  so empty string disables disk-utxo (in-memory mode)
# 3 GiB RocksDB cache per lane: at 800k+ heights the UTXO set is ~16 GB on disk; 512 MB
# gives ~3% hit rate (every prefetch hits NVMe).  3 GiB ≈ 19% hit rate — measurable BPS gain.
# 4 lanes × 3 GiB = 12 GiB; combined with 22 GiB RSS cap leaves headroom on 91 GiB.
export DISK_UTXO_ROCKSDB_CACHE_BYTES="${DISK_UTXO_ROCKSDB_CACHE_BYTES:-$((3*1024*1024*1024))}"
export DISK_UTXO_COMMIT_INTERVAL="${DISK_UTXO_COMMIT_INTERVAL:-500}"
export DISK_UTXO_OVERLAY_CAP="${DISK_UTXO_OVERLAY_CAP:-1000000}"
export DISK_UTXO_WRITE_BUFFER_MB="${DISK_UTXO_WRITE_BUFFER_MB:-64}"
# Immediately decommit freed mimalloc segments (v2 API; MIMALLOC_PAGE_RESET is deprecated in v2).
export MIMALLOC_PURGE_DELAY="${MIMALLOC_PURGE_DELAY:-0}"
export MIMALLOC_PAGE_RESET="${MIMALLOC_PAGE_RESET:-0}"
# Restrict glibc to a single malloc arena.  The default is up to 8×nCPU arenas; each arena
# independently ratchets its heap high-watermark from coin-cache flush cycles.  A single arena
# consolidates fragmentation and makes malloc_trim(0) far more effective at returning freed
# pages to the OS.  Drawback: thread-local allocation contention, but our 2 worker-thread
# workload is not allocation-heavy enough for this to matter.
export MALLOC_ARENA_MAX="${MALLOC_ARENA_MAX:-1}"
LOG_STEM="${LOG_STEM:-kernel_diff_0_500k_v2}"
WORKER_THREADS="${WORKER_THREADS:-2}"
COINS_CACHE_MB="${COINS_CACHE_MB:-1500}"
# RSS_LIMIT_MB: per-lane RSS cap.  With KERNEL_BLOCKTREE_RAM=0 and working --import-from-core-tip,
# restarts are cheap (~1 min seek-free via SeedHeadlessRestore), so this limit is safe to enable.
# 3 active lanes × 28 GiB = 84 GiB on a 91 GiB system leaves ~7 GiB buffer for the OS.
# Raised from 22000 to 28000 to reduce restart frequency: each restart costs 5–20 min of chunk
# seek time (despite SeedHeadlessRestore, the block archive still needs seeking).
RSS_LIMIT_MB="${RSS_LIMIT_MB:-28000}"
MEM_AVAILABLE_FLOOR_MB="${MEM_AVAILABLE_FLOOR_MB:-2000}"
MEM_AVAILABLE_RECOVERY_PASSES="${MEM_AVAILABLE_RECOVERY_PASSES:-3}"
MEM_AVAILABLE_RECOVERY_SLOP_MB="${MEM_AVAILABLE_RECOVERY_SLOP_MB:-250}"
# Write a UTXO checkpoint every N blocks so there's always a recent resume point.
CHUNK_CHECKPOINT_EVERY="${CHUNK_CHECKPOINT_EVERY:-25000}"
KERNEL_DIFF_PROGRESS_EVERY="${KERNEL_DIFF_PROGRESS_EVERY:-100}"
BINARY="${BLOCK_KERNEL_DIFF_BIN:-$SCRIPT_ROOT/target/release/block_kernel_diff}"

if [[ -z "${BITCOIN_CORE_LIB_DIR:-}" ]]; then
  echo "Set BITCOIN_CORE_LIB_DIR to the directory containing libbitcoinkernel.a (or .so)." >&2
  echo "Example: export BITCOIN_CORE_LIB_DIR=/path/to/bitcoin-core/build/lib" >&2
  exit 1
fi
export BITCOIN_CORE_LIB_DIR

RUNNER_LOG="$BLOCK_CACHE_DIR/${LOG_STEM}.runner.log"
JSONL_LOG="$BLOCK_CACHE_DIR/${LOG_STEM}.jsonl"
DIV_LOG="$BLOCK_CACHE_DIR/${LOG_STEM}.divergences.jsonl"
CKPT_SUBDIR="differential_checkpoints_fixed_v1"
END="${KERNEL_DIFF_END:-900001}"

if [[ ! -x "$BINARY" ]]; then
  echo "Missing binary: $BINARY (build with: cd blvm-bench && cargo build --release --features bitcoinkernel --bin block_kernel_diff)" >&2
  exit 1
fi

if [[ ! -f "$BLOCK_CACHE_DIR/chunks/chunks.meta" && ! -f "$BLOCK_CACHE_DIR/chunks.meta" ]]; then
  echo "Chunk cache not found under BLOCK_CACHE_DIR=$BLOCK_CACHE_DIR" >&2
  echo "Mount the drive or set BLOCK_CACHE_DIR to the directory containing chunks/chunks.meta" >&2
  exit 1
fi

# Kill any stale instance from a previous session.
# Set KERNEL_DIFF_PARALLEL=1 to skip this kill (parallel-lanes mode — each lane manages its own process).
if [[ "${KERNEL_DIFF_PARALLEL:-0}" != "1" ]]; then
  n_old="$(pgrep -f 'blvm-bench.*block_kernel_diff' 2>/dev/null | wc -l || true)"
  pkill -f 'blvm-bench.*block_kernel_diff' 2>/dev/null || true
  sleep 0.5
  if [[ "${n_old:-0}" -gt 1 ]]; then
    echo "warn: had $n_old block_kernel_diff processes — only one instance should run (RAM doubles)" >&2
  fi
fi

# ── Read the last known checkpoint position from the runner log ──────────────
read_checkpoint_from_log() {
  local log="$1"
  H=0; START=1; ASSUME=0
  if [[ ! -f "$log" ]]; then return; fi

  local last_resume
  last_resume="$(grep 'KERNEL_DIFF_RESUME.*--blvm-checkpoint-height' "$log" 2>/dev/null | tail -1 || true)"
  if [[ -n "$last_resume" ]]; then
    [[ "$last_resume" =~ blvm-checkpoint-height[[:space:]]+([0-9]+) ]] && H="${BASH_REMATCH[1]}"
    [[ "$last_resume" =~ --start[[:space:]]+([0-9]+) ]] && START="${BASH_REMATCH[1]}"
  fi

  # Fallback: infer from last PROGRESS line if no RESUME exists.
  if [[ "$H" -eq 0 ]]; then
    local last_prog
    last_prog="$(grep 'KERNEL_DIFF_PROGRESS' "$log" 2>/dev/null | tail -1 || true)"
    if [[ "$last_prog" =~ height=([0-9]+) ]]; then
      H="${BASH_REMATCH[1]}"
      START=$(( H + 1 ))
    fi
  fi

  # ASSUME is set after this function returns; see KERNEL_DIFF_ASSUME env var below.
  ASSUME=0
}

# ── Initial position ──────────────────────────────────────────────────────────
read_checkpoint_from_log "$RUNNER_LOG"

# BLVM: height < ASSUME skips script/signature checks (see blvm_consensus::connect_block).
ASSUME="${KERNEL_DIFF_ASSUME:-938343}"
if [[ -n "${KERNEL_DIFF_RESUME_HEIGHT:-}" ]]; then
  H="${KERNEL_DIFF_RESUME_HEIGHT}"
  START=$(( H + 1 ))
  echo "KERNEL_DIFF_RESUME_HEIGHT=$H → START=$START (overrides stale log resume until next loop re-read)" >&2
fi

echo "=== restart-kernel-diff-500k (loop mode) ===" >&2
echo "BLOCK_CACHE_DIR=$BLOCK_CACHE_DIR (chunks + utxo checkpoints)" >&2
echo "KERNEL_DIFF_NVME_ROOT=$KERNEL_DIFF_NVME_ROOT (Core + RocksDB only)" >&2
echo "CORE_DATADIR=$CORE_DATADIR" >&2
[[ -n "$CORE_BLOCKS_DIR" ]] && echo "CORE_BLOCKS_DIR=$CORE_BLOCKS_DIR" >&2 \
  || echo "CORE_BLOCKS_DIR=(default $CORE_DATADIR/blocks)" >&2
[[ -n "$DISK_UTXO_PATH" ]] && echo "DISK_UTXO_PATH=$DISK_UTXO_PATH (disk-backed BLVM UTXO)" >&2 \
  || echo "DISK_UTXO_PATH=(in-memory BLVM UTXO)" >&2
_bc_resolved="$(readlink -f "$BLOCK_CACHE_DIR" 2>/dev/null || echo "$BLOCK_CACHE_DIR")"
_cd_resolved="$(readlink -f "$CORE_DATADIR" 2>/dev/null || echo "$CORE_DATADIR")"
_du_resolved="$(readlink -f "$DISK_UTXO_PATH" 2>/dev/null || echo "$DISK_UTXO_PATH")"
if [[ "$_cd_resolved" == "$_bc_resolved"/* ]] || [[ "$_du_resolved" == "$_bc_resolved"/* ]]; then
  echo "WARNING: CORE_DATADIR or DISK_UTXO_PATH is under BLOCK_CACHE_DIR — keep databases on NVMe (set KERNEL_DIFF_NVME_ROOT or absolute CORE_DATADIR/DISK_UTXO_PATH)." >&2
fi
echo "initial resume: H=$H START=$START assume_valid=${ASSUME:-0} (last height target $((END - 1)))" >&2
MEM_NOTE="coins_cache=${COINS_CACHE_MB}MiB rocksdb_cache=$((DISK_UTXO_ROCKSDB_CACHE_BYTES/1048576))MiB overlay_cap=${DISK_UTXO_OVERLAY_CAP:-3M} rss_limit=${RSS_LIMIT_MB}MiB workers=$WORKER_THREADS commit_every=${DISK_UTXO_COMMIT_INTERVAL}blks purge_delay=${MIMALLOC_PURGE_DELAY}ms ckpt_every=${CHUNK_CHECKPOINT_EVERY}blks progress_every=${KERNEL_DIFF_PROGRESS_EVERY}blks skip_scripts=${KERNEL_SKIP_SCRIPTS:-1} blocktree_ram=${KERNEL_BLOCKTREE_RAM:-1} chainstate_ram=${KERNEL_CHAINSTATE_RAM:-0} arena_max=${MALLOC_ARENA_MAX:-1}"
[[ "$MEM_AVAILABLE_FLOOR_MB" != "0" ]] && MEM_NOTE+=" MemAvailable>=${MEM_AVAILABLE_FLOOR_MB}MiB (ok_if>=$((MEM_AVAILABLE_FLOOR_MB - MEM_AVAILABLE_RECOVERY_SLOP_MB))MiB) mem_recover=${MEM_AVAILABLE_RECOVERY_PASSES}pass slop=${MEM_AVAILABLE_RECOVERY_SLOP_MB}MiB"
echo "mem: $MEM_NOTE" >&2
echo "target: $((END - 1)) | runner log: $RUNNER_LOG (appending)" >&2

LOOP_ITER=0

# ── Main loop ─────────────────────────────────────────────────────────────────
while true; do
  LOOP_ITER=$(( LOOP_ITER + 1 ))

  # Already done? (e.g. log shows we reached END on a prior invocation)
  if [[ "$START" -ge "$END" ]]; then
    echo "=== already at target height $((START-1)) >= $((END-1)), nothing to do ===" >&2
    break
  fi

  EXTRA_FLAGS=()

  # Smart wipe: check whether the RocksDB is already at checkpoint height H.
  # The binary writes `DISK_UTXO_PATH/.chunk_utxo_disk_tip = H` when it saves a durable
  # checkpoint; skipping the RocksDB wipe saves the 200+ second rehydration of 44M entries.
  #
  # Core smart restore: `seed_headless_restore` rebuilds only the in-memory pprev stub chain from
  # headers (no 57M-UTXO reload) when `.kernel_diff_core_tip` matches H. This eliminates the
  # ~10–50 GiB glibc heap spike from cycling CCoinsViewCache on every restart.
  _DISK_TIP=""
  _SKIP_ROCKS_WIPE=0
  _CORE_TIP=""
  _SKIP_CORE_REIMPORT=0
  if [[ "$H" -gt 0 || "$START" -gt 1 ]]; then
    EXTRA_FLAGS+=(--blvm-prefer-utxo-snapshot)

    # ── Core tip check ──────────────────────────────────────────────────────────────────────────
    # If `.kernel_diff_core_tip` contains H the coins DB is already seeded from a previous run
    # that processed at least one block.  Use seed_headless_restore (headers-only) instead of
    # wiping + re-importing the full snapshot.
    _CORE_TIP_FILE="$CORE_DATADIR/.kernel_diff_core_tip"
    if [[ -f "$_CORE_TIP_FILE" ]]; then
      _CORE_TIP="$(cat "$_CORE_TIP_FILE" 2>/dev/null | tr -d '[:space:]' || true)"
    fi
    if [[ -d "$CORE_DATADIR/chainstate" ]] && [[ "$_CORE_TIP" == "$H" ]]; then
      echo "   [loop $LOOP_ITER] Core already at height $H (core_tip=$_CORE_TIP) — using seed_headless_restore (no UTXO reload)" >&2
      _SKIP_CORE_REIMPORT=1
      EXTRA_FLAGS+=(--import-from-core-tip)
    else
      if [[ -d "$CORE_DATADIR" ]]; then
        echo "   [loop $LOOP_ITER] wiping Core datadir $CORE_DATADIR (core_tip=${_CORE_TIP:-missing} need=$H)" >&2
        rm -rf "$CORE_DATADIR"
      fi
      EXTRA_FLAGS+=(--wipe-chainstate-db --import-from-deltas)
    fi

    if [[ -n "$DISK_UTXO_PATH" ]]; then
      _DISK_TIP="$(cat "$DISK_UTXO_PATH/.chunk_utxo_disk_tip" 2>/dev/null | tr -d '[:space:]' || true)"

      # RocksDB: only wipe if disk_tip doesn't match H (saves the 44M-entry reimport).
      if [[ -d "$DISK_UTXO_PATH" ]] && [[ "$_DISK_TIP" == "$H" ]]; then
        echo "   [loop $LOOP_ITER] RocksDB already at height $H (disk_tip=$_DISK_TIP) — skipping RocksDB wipe" >&2
        _SKIP_ROCKS_WIPE=1
      else
        if [[ -d "$DISK_UTXO_PATH" ]]; then
          echo "   [loop $LOOP_ITER] wiping RocksDB (disk_tip=${_DISK_TIP:-missing} need=$H)" >&2
          rm -rf "$DISK_UTXO_PATH"
        fi
      fi
    fi
  fi

  [[ "$ASSUME" -gt 0 ]] && EXTRA_FLAGS+=(--blvm-assume-valid-height "$ASSUME")

  mkdir -p "$CORE_DATADIR"
  [[ -n "$DISK_UTXO_PATH" ]] && mkdir -p "$DISK_UTXO_PATH"

  LIMIT_ARGS=(--worker-threads "$WORKER_THREADS" --kernel-coins-cache-mb "$COINS_CACHE_MB")
  [[ "$RSS_LIMIT_MB"            != "0" ]] && LIMIT_ARGS+=(--rss-limit-mb "$RSS_LIMIT_MB")
  [[ "$MEM_AVAILABLE_FLOOR_MB"  != "0" ]] && LIMIT_ARGS+=(--mem-available-floor-mb "$MEM_AVAILABLE_FLOOR_MB")
  LIMIT_ARGS+=(--mem-available-recovery-passes "$MEM_AVAILABLE_RECOVERY_PASSES")
  LIMIT_ARGS+=(--mem-available-recovery-slop-mb "$MEM_AVAILABLE_RECOVERY_SLOP_MB")
  [[ "$KERNEL_DIFF_PROGRESS_EVERY" != "0" ]] && LIMIT_ARGS+=(--progress-every "$KERNEL_DIFF_PROGRESS_EVERY")
  [[ -n "$CORE_BLOCKS_DIR"              ]] && LIMIT_ARGS+=(--core-blocks-dir "$CORE_BLOCKS_DIR")
  [[ -n "$DISK_UTXO_PATH"               ]] && LIMIT_ARGS+=(--disk-utxo-path "$DISK_UTXO_PATH")
  # KERNEL_CHAINSTATE_RAM=1 to put Core's chainstate in RAM (faster UTXO lookups but causes
  # LevelDB MemEnv to grow 2–3 GiB during comparison, triggering MemAvailable restarts).
  [[ "${KERNEL_CHAINSTATE_RAM:-0}" == "1" ]] && LIMIT_ARGS+=(--kernel-chainstate-ram)
  [[ "${KERNEL_BLOCKTREE_RAM:-0}"  == "1" ]] && LIMIT_ARGS+=(--kernel-blocktree-ram)
  # Skip all script evaluation in Core's kernel for this window (matches BLVM when height < ASSUME).
  [[ "${KERNEL_SKIP_SCRIPTS:-1}" == "1" ]] && LIMIT_ARGS+=(--kernel-skip-scripts)

  echo "   [loop $LOOP_ITER] start=$START end=$((END-1)) checkpoint_h=$H rss_limit=${RSS_LIMIT_MB}MB" >&2

  # Run the binary — NOT exec so we can loop. Capture pipe exit status via PIPESTATUS.
  set +e
  env BITCOIN_CORE_LIB_DIR="$BITCOIN_CORE_LIB_DIR" "$BINARY" \
    --block-cache-dir "$BLOCK_CACHE_DIR" \
    --core-datadir "$CORE_DATADIR" \
    --checkpoint-dir "$CKPT_SUBDIR" \
    --blvm-checkpoint-height "$H" \
    --start "$START" \
    --end "$END" \
    "${LIMIT_ARGS[@]}" \
    --checkpoint-every "$CHUNK_CHECKPOINT_EVERY" \
    --utxo-checkpoint-format fixed-v1 \
    "${EXTRA_FLAGS[@]}" \
    --jsonl-log "$JSONL_LOG" \
    --divergence-log "$DIV_LOG" \
    2>&1 | tee -a "$RUNNER_LOG"
  bin_exit="${PIPESTATUS[0]}"
  set -e

  # ── Check if we've reached the target ────────────────────────────────────────
  last_status="$(grep 'KERNEL_DIFF_STATUS' "$RUNNER_LOG" 2>/dev/null | tail -1 || true)"
  last_prog="$(grep 'KERNEL_DIFF_PROGRESS' "$RUNNER_LOG" 2>/dev/null | tail -1 || true)"
  reached_height=0
  if [[ "$last_prog" =~ height=([0-9]+) ]]; then
    reached_height="${BASH_REMATCH[1]}"
  fi

  if [[ "$last_status" =~ KERNEL_DIFF_STATUS[[:space:]]+(OK|DIVERGED) ]] || \
     [[ "$reached_height" -ge $(( END - 1 )) ]]; then
    echo "=== [loop $LOOP_ITER] completed at height $reached_height (status: $last_status) ===" >&2
    # Re-exit with the binary's exit code so the caller knows if there were divergences.
    exit "$bin_exit"
  fi

  # ── Not done yet — find the new checkpoint to resume from ────────────────────
  prev_H=$H
  read_checkpoint_from_log "$RUNNER_LOG"
  ASSUME="${KERNEL_DIFF_ASSUME:-938343}"

  # After the binary exits, the RocksDB disk_tip file already reflects the checkpoint it wrote.
  # Also persist the Core synced height so the next loop iteration can skip the wipe.
  # (The binary writes `.chunk_utxo_disk_tip` itself; we just echo the correlation here.)
  if [[ "$H" -gt 0 ]] && [[ -n "$DISK_UTXO_PATH" ]]; then
    _NEW_TIP="$(cat "$DISK_UTXO_PATH/.chunk_utxo_disk_tip" 2>/dev/null | tr -d '[:space:]' || true)"
    echo "   [loop $LOOP_ITER] disk_tip after run: ${_NEW_TIP:-unknown} (new resume H=$H)" >&2
  fi

  if [[ "$H" -le "$prev_H" && "$bin_exit" -ne 0 ]]; then
    # If we were using --import-from-core-tip and it failed, delete the sentinel so the next
    # iteration falls back to the safe wipe+reimport path (--import-from-deltas).
    # With KERNEL_BLOCKTREE_RAM=0, the block tree is now persisted to disk, so
    # --import-from-core-tip should succeed on the NEXT run after a wipe+reimport.
    # We allow at most one fallback per loop counter (don't loop-fallback indefinitely).
    _CORE_TIP_FILE="$CORE_DATADIR/.kernel_diff_core_tip"
    if [[ " ${EXTRA_FLAGS[*]} " == *" --import-from-core-tip "* ]] && [[ -f "$_CORE_TIP_FILE" ]]; then
      echo "   [loop $LOOP_ITER] --import-from-core-tip failed; wiping sentinel → next loop uses wipe+reimport" >&2
      rm -f "$_CORE_TIP_FILE"
      sleep 2
      continue
    fi
    echo "=== [loop $LOOP_ITER] ERROR: binary exited $bin_exit and no new checkpoint found (H stayed at $H). Aborting. ===" >&2
    exit 1
  fi

  echo "   [loop $LOOP_ITER] exited (code=$bin_exit) at height $reached_height → resuming from H=$H START=$START" >&2
  sleep 1
done
