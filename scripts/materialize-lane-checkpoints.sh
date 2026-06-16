#!/usr/bin/env bash
# Materialize lane checkpoints one at a time (bounded RAM per step).
set -euo pipefail

CP_DIR=/mnt/extra/blockchain/differential_checkpoints_fixed_v1
BIN=/mnt/data/bitcoin/blvm/blvm-bench/target/release/materialize_utxo_snapshots

materialize_one() {
  local from=$1 to=$2
  local dest="$CP_DIR/utxo_${to}.bin"
  if [[ -f "$dest" ]]; then
    echo "✓ utxo_${to}.bin already exists ($(du -h "$dest" | cut -f1))"
    return 0
  fi
  echo "=== utxo_${from} → utxo_${to} ==="
  "$BIN" --dir "$CP_DIR" --from "$from" --to "$to"
  echo "✅ $(du -h "$dest" | cut -f1) $dest"
  echo ""
}

cd /mnt/data/bitcoin/blvm/blvm-bench
cargo build --release --features utxo-snapshot-tools --bin materialize_utxo_snapshots 2>&1 | tail -5

materialize_one 500000 600000
materialize_one 600000 700000
materialize_one 700000 800000

echo "All lane checkpoints:"
ls -lh "$CP_DIR"/utxo_{500000,600000,700000,800000}.bin
