#!/usr/bin/env bash
# Orchestrator for kernel differential runs (plan Phase 4 + checkpoint ladder).
#
# Subcommands:
#   prep-chunks  — validate chunk cache + index before chunk_utxo_checkpoints (contiguous chain, chunk files).
#                  Build: cargo build --release --features scan --bin prep_chunk_utxo_cache
#   from-chunks  — BLVM only: walk chunk cache, connect_block, write utxo_*.bin (no Core / libbitcoinkernel).
#                  Build: cargo build --release --features "scan,disk-utxo" --bin chunk_utxo_checkpoints
#   checkpoints  — block_kernel_diff (BLVM + Core); needs BITCOIN_CORE_LIB_DIR.
#   parallel     — N [start,end) lanes; separate Core datadirs (kernel-diff-parallel-lanes.sh).
#   help         — this text
#
# Defaults match block_kernel_diff: ~/.local/share/blvm-kernel-diff/{chunk-cache,core-datadir}
# unless BLOCK_CACHE_DIR / CORE_DIFF_DATADIR / --block-cache-dir / --core-datadir are set.
#
# Build from-chunks: cargo build --release --features "scan,disk-utxo" --bin chunk_utxo_checkpoints
# Build checkpoints:  cargo build --release --features bitcoinkernel --bin block_kernel_diff
# Link:  export BITCOIN_CORE_LIB_DIR=/path/to/dir/with/libbitcoinkernel.a

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BLOCK_KERNEL_DIFF_BIN:-$ROOT/target/release/block_kernel_diff}"
CHUNK_BIN="${CHUNK_UTXO_CHECKPOINTS_BIN:-$ROOT/target/release/chunk_utxo_checkpoints}"
PREP_BIN="${PREP_CHUNK_UTXO_CACHE_BIN:-$ROOT/target/release/prep_chunk_utxo_cache}"
PARALLEL_SCRIPT="$ROOT/scripts/kernel-diff-parallel-lanes.sh"

usage() {
  cat <<EOF
Orchestrator for kernel differential runs (checkpoint ladder + parallel lanes).

Usage:
  $0 prep-chunks [--rebuild-index] [--min-contiguous N] [--start H] [--end H] [-- EXTRA...]
  $0 from-chunks --every N [--start H] [--end H] [--format bincode|fixed-v1] [-- EXTRA...]
  $0 checkpoints --every N [--start H] [--end H] [--format bincode|fixed-v1] [-- EXTRA...]
  $0 parallel <start0> <end0> [start1 end1 ...]
  $0 help

Checkpoints: {BLOCK_CACHE_DIR}/differential_checkpoints/utxo_<height>.bin

Examples:
  $0 prep-chunks
  $0 from-chunks --every 50000 --start 0 --end 500000 --format fixed-v1
  $0 checkpoints --every 50000 --start 0 --end 500000 --format fixed-v1
  $0 checkpoints --every 100000 --start 200000 --end 400000 --format bincode -- --worker-threads 6
EOF
  exit "${1:-0}"
}

run_checkpoints() {
  local EVERY=""
  local START="0"
  local END=""
  local FORMAT="bincode"
  local -a EXTRA=()
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == "--" ]]; then
      shift
      EXTRA+=("$@")
      break
    fi
    case "$1" in
      --every)
        EVERY="${2:?}"
        shift 2
        ;;
      --start)
        START="${2:?}"
        shift 2
        ;;
      --end)
        END="${2:?}"
        shift 2
        ;;
      --format)
        FORMAT="${2:?}"
        shift 2
        ;;
      *)
        echo "Unknown option: $1 (use -- before pass-through args)" >&2
        usage 1
        ;;
    esac
  done

  if [[ -z "$EVERY" || "$EVERY" -le 0 ]]; then
    echo "checkpoints: --every N (positive) is required" >&2
    exit 1
  fi
  case "$FORMAT" in
    bincode|fixed-v1) ;;
    *)
      echo "checkpoints: --format must be bincode or fixed-v1" >&2
      exit 1
      ;;
  esac

  if [[ ! -x "$BIN" && ! -f "$BIN" ]]; then
    echo "block_kernel_diff not found at $BIN — build:" >&2
    echo "  cargo build --release --features bitcoinkernel --bin block_kernel_diff" >&2
    exit 1
  fi

  local -a CMD=("$BIN" --start "$START" --checkpoint-every "$EVERY" --utxo-checkpoint-format "$FORMAT")
  if [[ -n "$END" ]]; then
    CMD+=(--end "$END")
  fi
  CMD+=("${EXTRA[@]}")

  echo "=== kernel-diff orchestrator: checkpoints ==="
  echo "  --every $EVERY --start $START ${END:+--end $END} --utxo-checkpoint-format $FORMAT"
  echo "  Output: {BLOCK_CACHE_DIR}/differential_checkpoints/utxo_<height>.bin"
  echo "  BIN=$BIN"
  echo ""

  exec "${CMD[@]}"
}

run_from_chunks() {
  local EVERY=""
  local START="0"
  local END=""
  local FORMAT="bincode"
  local -a EXTRA=()
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == "--" ]]; then
      shift
      EXTRA+=("$@")
      break
    fi
    case "$1" in
      --every)
        EVERY="${2:?}"
        shift 2
        ;;
      --start)
        START="${2:?}"
        shift 2
        ;;
      --end)
        END="${2:?}"
        shift 2
        ;;
      --format)
        FORMAT="${2:?}"
        shift 2
        ;;
      *)
        echo "Unknown option: $1 (use -- before pass-through args)" >&2
        usage 1
        ;;
    esac
  done

  if [[ -z "$EVERY" || "$EVERY" -le 0 ]]; then
    echo "from-chunks: --every N (positive) is required" >&2
    exit 1
  fi
  case "$FORMAT" in
    bincode|fixed-v1) ;;
    *)
      echo "from-chunks: --format must be bincode or fixed-v1" >&2
      exit 1
      ;;
  esac

  if [[ ! -x "$CHUNK_BIN" && ! -f "$CHUNK_BIN" ]]; then
    echo "chunk_utxo_checkpoints not found at $CHUNK_BIN — build:" >&2
    echo "  cargo build --release --features \"scan,disk-utxo\" --bin chunk_utxo_checkpoints" >&2
    exit 1
  fi

  local -a CMD=("$CHUNK_BIN" --start "$START" --checkpoint-every "$EVERY" --format "$FORMAT")
  if [[ -n "$END" ]]; then
    CMD+=(--end "$END")
  fi
  CMD+=("${EXTRA[@]}")

  echo "=== kernel-diff orchestrator: from-chunks (BLVM only, no Core) ==="
  echo "  --every $EVERY --start $START ${END:+--end $END} --format $FORMAT"
  echo "  Output: {BLOCK_CACHE_DIR}/differential_checkpoints/utxo_<height>.bin"
  echo "  BIN=$CHUNK_BIN"
  echo ""

  exec "${CMD[@]}"
}

run_prep_chunks() {
  if [[ ! -x "$PREP_BIN" && ! -f "$PREP_BIN" ]]; then
    echo "prep_chunk_utxo_cache not found at $PREP_BIN — build:" >&2
    echo "  cargo build --release --features scan --bin prep_chunk_utxo_cache" >&2
    exit 1
  fi
  echo "=== kernel-diff orchestrator: prep-chunks (chunk cache + index) ==="
  echo "  BIN=$PREP_BIN"
  echo ""
  exec "$PREP_BIN" "$@"
}

case "${1:-help}" in
  prep-chunks)
    shift
    run_prep_chunks "$@"
    ;;
  from-chunks)
    shift
    run_from_chunks "$@"
    ;;
  checkpoints)
    shift
    run_checkpoints "$@"
    ;;
  parallel)
    shift
    if [[ ! -x "$PARALLEL_SCRIPT" ]]; then
      echo "Missing $PARALLEL_SCRIPT" >&2
      exit 1
    fi
    exec "$PARALLEL_SCRIPT" "$@"
    ;;
  help|-h|--help|"")
    usage 0
    ;;
  *)
    echo "Unknown subcommand: $1" >&2
    usage 1
    ;;
esac
