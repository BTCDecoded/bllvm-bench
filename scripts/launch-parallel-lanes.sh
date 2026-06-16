#!/usr/bin/env bash
# Launch 4 parallel kernel-diff lanes covering 500k–912724.
# REQUIRES utxo_{500,600,700,800}000.bin to exist — run materialize-lane-checkpoints.sh first.
# Never materializes in parallel (that OOMs). Each lane streams utxo_H.bin into NVMe RocksDB.
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CP_DIR=/mnt/extra/blockchain/differential_checkpoints_fixed_v1
LANE_BASE=/mnt/data/blvm-kernel-diff/lanes
LOG_BASE=/mnt/extra/blockchain

for h in 500000 600000 700000 800000; do
  if [[ ! -f "$CP_DIR/utxo_${h}.bin" ]]; then
    echo "ERROR: missing $CP_DIR/utxo_${h}.bin" >&2
    echo "Run: bash $SCRIPT_ROOT/scripts/materialize-lane-checkpoints.sh" >&2
    exit 1
  fi
done

# Kill any stale single-lane or partial parallel runs
pkill -f 'blvm-bench.*block_kernel_diff' 2>/dev/null || true
pkill -f 'restart-kernel-diff-500k' 2>/dev/null || true
sleep 2

launch_lane() {
  local i=$1 cp=$2 end=$3
  local lane_dir="$LANE_BASE/lane${i}"
  local stem="lane${i}_${cp}_${end}"
  rm -rf "$lane_dir/core" "$lane_dir/du"
  mkdir -p "$lane_dir/core" "$lane_dir/du"

  echo "Launching lane $i: blocks $((cp+1))–$((end-1))"

  LOG_STEM="$stem" \
  KERNEL_DIFF_RESUME_HEIGHT="$cp" \
  KERNEL_DIFF_END="$end" \
  CORE_DATADIR="$lane_dir/core" \
  DISK_UTXO_PATH="$lane_dir/du" \
  COINS_CACHE_MB=750 \
  WORKER_THREADS=2 \
  MEM_AVAILABLE_FLOOR_MB=3000 \
  DISK_UTXO_ROCKSDB_CACHE_BYTES=$((512*1024*1024)) \
  DISK_UTXO_OVERLAY_CAP=1000000 \
  DISK_UTXO_COMMIT_INTERVAL=2000 \
  KERNEL_DIFF_PARALLEL=1 \
  nohup bash "$SCRIPT_ROOT/scripts/restart-kernel-diff-500k.sh" \
    >> "$LOG_BASE/${stem}.wrapper.log" 2>&1 &

  echo "  Lane $i PID=$!"
}

launch_lane 0 500000 600001
sleep 30   # stagger RocksDB imports — each loads ~60-100M entries
launch_lane 1 600000 700001
sleep 30
launch_lane 2 700000 800001
sleep 30
launch_lane 3 800000 912725

echo ""
echo "All 4 lanes launched (staggered 30s). Monitor:"
echo "  grep KERNEL_DIFF_PROGRESS $LOG_BASE/lane*_*.runner.log | tail -4"
