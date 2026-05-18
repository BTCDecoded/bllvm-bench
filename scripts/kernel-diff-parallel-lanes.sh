#!/usr/bin/env bash
# Run N non-overlapping height windows in parallel using BLVM delta-ladder + Core headless seeding.
#
# ── Flow per lane ──────────────────────────────────────────────────────────────────────────────
#  1. Create an empty per-lane Core datadir (no block index needed).
#  2. Run block_kernel_diff with --import-from-deltas for that lane.
#     The binary materializes utxo_H.bin from utxo_0 + delta ladder (if absent), reads the
#     last 11 block headers from the chunk cache, calls seed_headless to create synthetic Core
#     chain stubs at height H, then compares blocks [H+1, end) against BLVM (which also
#     reconstructs from deltas in-memory).
#  3. All lanes run concurrently; failures are collected and reported at the end.
#
# ── No bootstrap required ────────────────────────────────────────────────────────────────────
#  Unlike the old wipe+import flow, --import-from-deltas uses SeedHeadlessChainstate in the Core
#  fork which creates synthetic CBlockIndex stubs from the provided block headers.  No pre-built
#  LevelDB block tree / block index is needed.  Each lane gets a fresh empty datadir.
#
# ── Usage ─────────────────────────────────────────────────────────────────────────────────────
#  kernel-diff-parallel-lanes.sh <LANE0_CHECKPOINT> <LANE0_END> <LANE1_CHECKPOINT> <LANE1_END> …
#
#  Each pair: checkpoint height H (BLVM + Core seeded to H, compare H+1..end).
#  Heights should align with the delta ladder (multiples of 100k).
#
#  Example — 9 parallel lanes covering 0–900k in 100k bands:
#    kernel-diff-parallel-lanes.sh \
#      0 100000  100000 200000  200000 300000  300000 400000  400000 500000 \
#      500000 600000  600000 700000  700000 800000  800000 900000
#
# ── Spotting divergences ─────────────────────────────────────────────────────────────────────
#  Each lane writes **only** mismatches to lane*-h*.divergences.jsonl (easy `wc -l`).
#  Logs also contain stderr markers: KERNEL_DIFF_DIVERGENCE, KERNEL_DIFF_SUMMARY, KERNEL_DIFF_STATUS.
#  Full .log mixes JSONL (stdout) + stderr; for jq on full log use:
#    grep '^{' "$LANE_WORK_DIR"/*.log | jq -c 'select(.divergence==true)'
#
# ── Env ───────────────────────────────────────────────────────────────────────────────────────
#  BLOCK_CACHE_DIR        Chunk cache root (blocks + delta checkpoints).
#  UTXO_CHECKPOINT_DIR    Subdir inside BLOCK_CACHE_DIR for delta files
#                         (default: differential_checkpoints_fixed_v1).
#  BLOCK_KERNEL_DIFF_BIN  Override binary path.
#  LANE_WORK_DIR          Parent dir for per-lane Core datadirs
#                         (default: ~/.local/share/blvm-kernel-diff/lanes).
#  CORE_DIFF_DATADIR_LANE_${i}  Override per-lane datadir (optional).

set -euo pipefail

if (( $# < 2 || $# % 2 != 0 )); then
  echo "Usage: $0 <checkpoint0> <end0> [checkpoint1 end1 ...]" >&2
  echo "Each pair: checkpoint height H (lane seeded at H, compares H+1..end)." >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BLOCK_KERNEL_DIFF_BIN:-$ROOT/target/release/block_kernel_diff}"
if [[ ! -x "$BIN" ]]; then
  echo "block_kernel_diff not found / not executable at $BIN" >&2
  echo "Build with: cargo build --release --features bitcoinkernel -p blvm-bench" >&2
  exit 1
fi

LANE_WORK_DIR="${LANE_WORK_DIR:-${HOME}/.local/share/blvm-kernel-diff/lanes}"
UTXO_CHECKPOINT_DIR="${UTXO_CHECKPOINT_DIR:-differential_checkpoints_fixed_v1}"

LANES=$(( $# / 2 ))
ARGS=("$@")
PIDS=()
LANE_LOGS=()

mkdir -p "${LANE_WORK_DIR}"
echo "Launching $LANES parallel lanes (headless, no block index required) …"

i=0
while (( i < LANES )); do
  CP="${ARGS[$((i * 2))]}"
  END="${ARGS[$((i * 2 + 1))]}"
  START=$(( CP + 1 ))

  VAR="CORE_DIFF_DATADIR_LANE_${i}"
  LANE_DIR="${!VAR:-${LANE_WORK_DIR}/lane${i}-h${CP}}"
  LOG_FILE="${LANE_WORK_DIR}/lane${i}-h${CP}.log"
  DIV_FILE="${LANE_WORK_DIR}/lane${i}-h${CP}.divergences.jsonl"
  LANE_LOGS+=("$LOG_FILE")

  # Create empty lane datadir (no block index copy needed for headless path).
  mkdir -p "${LANE_DIR}/blocks"

  echo "  lane $i: CP=$CP END=$END dir=${LANE_DIR} log=${LOG_FILE}"

  # ── Launch block_kernel_diff ─────────────────────────────────────────────────────────────────
  (
    BLOCK_CACHE_DIR="${BLOCK_CACHE_DIR:-}" \
    CORE_DIFF_DATADIR="${LANE_DIR}" \
    UTXO_CHECKPOINT_DIR="${UTXO_CHECKPOINT_DIR}" \
    "$BIN" \
      --blvm-checkpoint-height "${CP}" \
      --start "${START}" \
      --end "${END}" \
      --import-from-deltas \
      --divergence-log "${DIV_FILE}" \
      > "${LOG_FILE}" 2>&1
  ) &
  PIDS+=($!)

  i=$(( i + 1 ))
done

echo "All $LANES lanes started. Waiting …"

ec=0
for idx in "${!PIDS[@]}"; do
  pid="${PIDS[$idx]}"
  log="${LANE_LOGS[$idx]}"
  if wait "$pid"; then
    echo "  lane $idx: DONE  (log: $log)"
  else
    echo "  lane $idx: FAILED (log: $log)"
    ec=1
  fi
done

if (( ec != 0 )); then
  echo "One or more lanes failed — check logs above." >&2
fi

# Divergence roll-up: nonzero line count in *.divergences.jsonl means that lane disagreed.
div_lines=0
shopt -s nullglob
for df in "${LANE_WORK_DIR}"/lane*-h*.divergences.jsonl; do
  [[ -f "$df" ]] || continue
  n=$(wc -l <"$df" | tr -d ' ')
  if [[ "$n" -gt 0 ]]; then
    echo "  KERNEL_DIFF_LANE_DIVERGENCES $n line(s) in ${df}"
    div_lines=$((div_lines + n))
  fi
done
shopt -u nullglob

any_div=false
shopt -s nullglob
for lf in "${LANE_WORK_DIR}"/*.log; do
  if grep -q "KERNEL_DIFF_STATUS DIVERGED" "$lf" 2>/dev/null; then
    any_div=true
    break
  fi
done
shopt -u nullglob
if [[ "$any_div" == true ]]; then
  echo "KERNEL_DIFF_LANES_STATUS: at least one lane reported DIVERGED (grep logs for KERNEL_DIFF_DIVERGENCE)" >&2
fi

if [[ "$div_lines" -gt 0 ]]; then
  echo "KERNEL_DIFF_LANES_SUMMARY total_divergence_records=${div_lines} (jq: jq -c 'select(.divergence==true)' ${LANE_WORK_DIR}/*.divergences.jsonl)" >&2
fi

exit "$ec"
