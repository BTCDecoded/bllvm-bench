#!/usr/bin/env bash
# Phase 0: fixed-height window for block_kernel_diff timing (plan §5).
# Usage: kernel-diff-benchmark-window.sh <START> <END>
# Optional env: BLOCK_CACHE_DIR, CORE_DIFF_DATADIR (defaults under ~/.local/share/blvm-kernel-diff/).
# Build needs BITCOIN_CORE_LIB_DIR when linking libbitcoinkernel.

set -euo pipefail
if [[ "${1:-}" == "" || "${2:-}" == "" ]]; then
  echo "Usage: $0 <START_HEIGHT> <END_HEIGHT>" >&2
  echo "Defaults: chunk cache and Core datadir under ~/.local/share/blvm-kernel-diff/ (see block_kernel_diff --help)." >&2
  exit 1
fi

START="$1"
END="$2"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BLOCK_KERNEL_DIFF_BIN:-$ROOT/target/release/block_kernel_diff}"

if [[ ! -x "$BIN" && ! -f "$BIN" ]]; then
  echo "block_kernel_diff not found at $BIN — set BLOCK_KERNEL_DIFF_BIN or build:" >&2
  echo "  cargo build --release --features bitcoinkernel --bin block_kernel_diff" >&2
  exit 1
fi

export START_HEIGHT="$START"
export END_HEIGHT="$END"

echo "=== kernel-diff benchmark window START=$START END=$END ==="
echo "BLOCK_CACHE_DIR=${BLOCK_CACHE_DIR:-<default resolve in binary>}"
echo "CORE_DIFF_DATADIR=${CORE_DIFF_DATADIR:-<default ~/.local/share/blvm-kernel-diff/core-datadir>}"
echo "BIN=$BIN"
echo ""

/usr/bin/time -v "$BIN" --start "$START" --end "$END"
