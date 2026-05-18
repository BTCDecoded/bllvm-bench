#!/usr/bin/env bash
# Run chunk_utxo_checkpoints in segments: stop after a given height, or resume from an on-disk checkpoint.
#
# Requires: cargo build --release --features "scan,disk-utxo" --bin chunk_utxo_checkpoints
# Optional: add low-mem-alloc for mimalloc in chunk_utxo_checkpoints (see binary source).
#
# Env (typical):
#   export BLOCK_CACHE_DIR=/path/to/blockchain
#   export BLVM_ASSUME_VALID_HEIGHT=1000000   # above max height you will process — large speedup (skip script verify below this height)
#   CHECKPOINT_EVERY=50000          # must match your ladder
#   FORMAT=fixed-v1                 # or bincode
#   UTXO_CHECKPOINT_DIR=differential_checkpoints_fixed_v1
#
# Large checkpoints (e.g. utxo_450000.bin): the in-memory UTXO alone is multi‑GiB. For 16–32 GiB RAM:
#   CHUNK_UTXO_LOW_MEM=1   # recommended: caps worker pools + serializes fixed-v1 **encode** (avoids sort spike)
#   CHUNK_UTXO_MAX_THREADS=4   # default when LOW_MEM=1 (override 2–8; capped by nproc). Connect path uses this;
#                                leaving everything at 1 thread made ~10–15 BPS; 4× is typical without huge RSS jump.
# Unset LOW_MEM for full Rayon/core count + default parallel encode (needs RAM headroom).
#
# Memory guard tuning (chunk_utxo_checkpoints; see src/bin/chunk_utxo_checkpoints.rs):
#   CHUNK_UTXO_MEM_GUARD_GB=0.5        # binary default when unset (decimal OK). 0 = MemAvailable exit off
#                                      # (optional CHUNK_UTXO_MEM_GUARD_MAX_RSS_GB still applies).
#   CHUNK_UTXO_MEM_WARN_GB=2           # default when unset: sample every block while MemAvailable is below this;
#                                      # 0 = only use CHUNK_UTXO_MEM_CHECK_BLOCKS interval.
#   CHUNK_UTXO_MEM_GUARD_MAX_RSS_GB=0  # unset/0 = off. VmRSS above limit → same clean exit as MemAvailable guard.
#   CHUNK_UTXO_MEM_CHECK_BLOCKS=200    # blocks between samples when not in “warn” fast mode (default 200)
#   CHUNK_UTXO_MEM_GUARD_CONSECUTIVE=1 # consecutive low MemAvailable samples before exit (default 1)
#
# Disk UTXO (binary built with --features disk-utxo; see src/disk_utxo.rs):
#   DISK_UTXO_ROCKSDB_CACHE_BYTES / DISK_UTXO_REDB_CACHE_BYTES  # block cache (default ~3 GiB)
#   DISK_UTXO_REHYDRATE_READ_BUF_MB=16     # larger read buffer if checkpoint .bin is on a slow disk
#   DISK_UTXO_REHYDRATE_BATCH / SINGLE_TX / STATUS_EVERY — documented in disk_utxo.rs module header
#
# Usage:
#   ./scripts/chunk-utxo-checkpoint-segments.sh help
#
#   # Process genesis through block 250000 (inclusive), then exit. Writes utxo_250000.bin when 250000 % EVERY == 0.
#   ./scripts/chunk-utxo-checkpoint-segments.sh run-to 250000
#
#   # Same but only from block 200001 (you already have utxo_200000.bin; do NOT use with run-to from 0)
#   START=200001 ./scripts/chunk-utxo-checkpoint-segments.sh run-to 250000
#
#   # Load utxo_250000.bin and continue from block 250001 (no duplicate work from genesis)
#   ./scripts/chunk-utxo-checkpoint-segments.sh resume-from 250000
#
#   # Optional final height (exclusive): stop before that block
#   ./scripts/chunk-utxo-checkpoint-segments.sh resume-from 250000 --end 500000
#
# Stopping cleanly at 250k without --end: use run-to 250000, or send SIGINT after you see
# "wrote ... utxo_250000.bin" in the log.
#
# Extra args after the subcommand are passed to chunk_utxo_checkpoints (e.g. --verify-chunk-block-hashes).

set -euo pipefail

if [[ "${CHUNK_UTXO_LOW_MEM:-}" == "1" ]]; then
  # OOM-safe defaults: moderate parallelism (not 1× — too slow after assume-valid). Hash prealloc + mem guard
  # fix the worst spikes; worker pools need threads for overlay/script batching throughput.
  _nproc="$(nproc 2>/dev/null || echo 4)"
  _cap="${CHUNK_UTXO_MAX_THREADS:-4}"
  [[ "$_cap" =~ ^[0-9]+$ ]] || _cap=4
  ((_cap < 1)) && _cap=1
  ((_cap > _nproc)) && _cap="$_nproc"
  export RAYON_NUM_THREADS="${RAYON_NUM_THREADS:-$_cap}"
  export BLVM_SCRIPT_WORKERS="${BLVM_SCRIPT_WORKERS:-$RAYON_NUM_THREADS}"
  export BLVM_CRYPTO_DRAIN_THREADS="${BLVM_CRYPTO_DRAIN_THREADS:-$RAYON_NUM_THREADS}"
  # glibc arenas: 2 limits fragmentation without starving 4 worker threads.
  export MALLOC_ARENA_MAX="${MALLOC_ARENA_MAX:-2}"
  # utxo_snapshot_fixed_v1: serial sort on encode — avoids Rayon sort RSS spike when writing utxo_H.bin.
  export CHUNK_UTXO_SERIAL_ENCODE="${CHUNK_UTXO_SERIAL_ENCODE:-1}"
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${CHUNK_UTXO_CHECKPOINTS_BIN:-$ROOT/target/release/chunk_utxo_checkpoints}"
EVERY="${CHECKPOINT_EVERY:-50000}"
FORMAT="${FORMAT:-fixed-v1}"
CHECKPOINT_DIR="${UTXO_CHECKPOINT_DIR:-differential_checkpoints_fixed_v1}"
START="${START:-0}"

usage() {
  sed -n '2,30p' "$0" | sed 's/^# \?//'
  exit "${1:-0}"
}

if [[ ! -f "$BIN" ]]; then
  echo "Missing $BIN — build: cargo build --release --features \"scan,disk-utxo\" --bin chunk_utxo_checkpoints (optional: ,low-mem-alloc)" >&2
  exit 1
fi

case "${1:-}" in
  help|-h|--help|"")
    usage 0
    ;;
  run-to)
    TARGET="${2:?need target block height (inclusive)}"
    shift 2
    # Process blocks [START, TARGET] inclusive → loop uses exclusive end TARGET+1
    END=$((TARGET + 1))
    if (( START >= END )); then
      echo "START ($START) must be < target block ($TARGET)" >&2
      exit 1
    fi
    exec "$BIN" \
      --start "$START" \
      --end "$END" \
      --checkpoint-every "$EVERY" \
      --format "$FORMAT" \
      --checkpoint-dir "$CHECKPOINT_DIR" \
      "$@"
    ;;
  resume-from)
    H="${2:?need checkpoint height H (loads utxo_H.bin; starts at H+1)}"
    shift 2
    # Loop: auto-restart after memory-guard clean exits. Each restart finds the
    # highest utxo_*.bin already written and resumes from there.
    CACHE_DIR="${BLOCK_CACHE_DIR:-}"
    if [[ -z "$CACHE_DIR" ]]; then
      echo "BLOCK_CACHE_DIR must be set for resume-from" >&2
      exit 1
    fi
    CKPT_FULL_DIR="$CACHE_DIR/$CHECKPOINT_DIR"
    DISK_DB="${DISK_UTXO_DB_PATH:-${XDG_CACHE_HOME:-$HOME/.cache}/blvm-bench/utxo_rocksdb}"
    DISK_TIP_FILE="${CHUNK_UTXO_DISK_TIP_FILE:-$CACHE_DIR/.chunk_utxo_disk_tip}"

    # Find the latest existing checkpoint height >= H
    latest_checkpoint_height() {
      find "$CKPT_FULL_DIR" -maxdepth 1 -name 'utxo_*.bin' 2>/dev/null \
        | sed 's|.*/utxo_\([0-9]*\)\.bin|\1|' \
        | sort -n \
        | tail -1
    }

    CURRENT_H="$H"
    RESTART_COUNT=0
    while true; do
      # Find the latest checkpoint >= the original H
      LATEST=$(latest_checkpoint_height)
      if [[ -n "$LATEST" && "$LATEST" -ge "$CURRENT_H" ]]; then
        CURRENT_H="$LATEST"
      fi

      # Warm RocksDB: `.chunk_utxo_disk_tip` holds the **durable** tip (same WriteBatch as UTXO rows).
      # Do **not** use `.chunk_utxo_checkpoints.status` here — it tracks last **connected** height for BPS
      # and can be ahead of what's on disk mid-overlay (crash = corruption if used as resume base).
      if [[ -f "$DISK_DB/CURRENT" ]]; then
        if [[ -f "$DISK_TIP_FILE" ]]; then
          read -r DISK_TIP < "$DISK_TIP_FILE" || DISK_TIP=""
          if [[ -n "${DISK_TIP:-}" ]] && [[ "$DISK_TIP" =~ ^[0-9]+$ ]] && [[ "$DISK_TIP" -gt "$CURRENT_H" ]]; then
            CURRENT_H="$DISK_TIP"
          fi
        fi
      fi

      START=$((CURRENT_H + 1))
      if [[ "$RESTART_COUNT" -gt 0 ]]; then
        echo "   [mem-guard restart #${RESTART_COUNT}] resuming from checkpoint height ${CURRENT_H}" >&2
      fi

      # Use stdbuf to force line-buffered stderr/stdout — without this, nohup fully buffers
      # all output until ~8KB, so status files written by the binary (compact, rehydr, etc.)
      # never appear in the log until the buffer fills.
      _LAUNCH="$BIN"
      if command -v stdbuf >/dev/null 2>&1; then
        _LAUNCH="stdbuf -oL -eL $BIN"
      fi
      $_LAUNCH \
        --blvm-checkpoint-height "$CURRENT_H" \
        --start "$START" \
        --checkpoint-every "$EVERY" \
        --format "$FORMAT" \
        --checkpoint-dir "$CHECKPOINT_DIR" \
        "$@"
      # set -e: if binary exits non-zero (real error), the script exits here automatically.
      # Reaching here means exit code 0 — either normal completion or memory-guard exit.

      # Check if a memory-guard exit wrote a new checkpoint to resume from.
      NEW_LATEST=$(latest_checkpoint_height)
      if [[ -z "$NEW_LATEST" || "$NEW_LATEST" -le "$CURRENT_H" ]]; then
        echo "   Finished: no new checkpoint after height ${CURRENT_H}." >&2
        break
      fi

      RESTART_COUNT=$((RESTART_COUNT + 1))
      CURRENT_H="$NEW_LATEST"
    done
    ;;
  *)
    echo "Unknown command: $1" >&2
    usage 1
    ;;
esac
