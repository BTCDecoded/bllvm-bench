#!/usr/bin/env bash
# Automated BPS test for kernel-diff-status.sh (no real chunk_utxo_checkpoints required).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
STATUS="$ROOT/kernel-diff-status.sh"
[[ -f "$STATUS" ]] || { echo "missing $STATUS" >&2; exit 2; }

fail() { echo "FAIL: $*" >&2; exit 1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$TMP/differential_checkpoints_fixed_v1"
# So disk=highest has a defined ladder (avoid empty glob issues)
: >"$TMP/differential_checkpoints_fixed_v1/utxo_500000.bin"

export BLOCK_CACHE_DIR="$TMP"
export KERNEL_DIFF_STATUS_MOCK_RUNNING=1
export KERNEL_DIFF_STATUS_MOCK_PID=424242
export KERNEL_DIFF_STATUS_POLL_STATE_FILE="$TMP/poll.state"
export UTXO_CHECKPOINT_DIR=differential_checkpoints_fixed_v1

# 1) First sample: establishes state; no prior poll — bps must be "-" (not bogus 0).
echo "1000000 1" >"$TMP/.chunk_utxo_checkpoints.status"
export KERNEL_DIFF_STATUS_MOCK_NOW=2000000
out1="$("$STATUS" --once checkpoints 2>&1)" || fail "script exit 1 on first poll"
echo "$out1" | grep -q 'bps=-' || fail "expected bps=- on first poll, got: $out1"

# 2) +30 blocks in +3 wall seconds => 10.00 BPS
echo "1000030 1" >"$TMP/.chunk_utxo_checkpoints.status"
export KERNEL_DIFF_STATUS_MOCK_NOW=2000003
out2="$("$STATUS" --once checkpoints 2>&1)" || fail "script exit 1 on second poll"
echo "$out2" | grep -q 'bps=10.00' || fail "expected bps=10.00, got: $out2"

# 3) Flat height (seek stall): must not print bps=0.00; keep last as ~10.00
echo "1000030 1" >"$TMP/.chunk_utxo_checkpoints.status"
export KERNEL_DIFF_STATUS_MOCK_NOW=2000010
out3="$("$STATUS" --once checkpoints 2>&1)" || fail "script exit 1 on third poll"
echo "$out3" | grep -qE 'bps=~10\.00' || fail "expected bps=~10.00 when stalled, got: $out3"

# 4) New poll state from a different mock pid must reset (no insane delta)
export KERNEL_DIFF_STATUS_MOCK_PID=555555
echo "1000030 1" >"$TMP/.chunk_utxo_checkpoints.status"
export KERNEL_DIFF_STATUS_MOCK_NOW=2000020
out4="$("$STATUS" --once checkpoints 2>&1)" || fail "script exit on pid change"
echo "$out4" | grep -qE 'bps=~10\.00|bps=-' || fail "after pid change expect ~last or -, got: $out4"

# 5) Rehydration: bps= shows UTXO rows/s from [disk-utxo] commit log line + windowed state
export KERNEL_DIFF_STATUS_REHYDR_STATE_FILE="$TMP/rehydr.state"
export CHUNK_UTXO_CHECKPOINT_LOG="$TMP/chunk.log"
export CHUNK_UTXO_REHYDRATE_STATUS_FILE="$TMP/.chunk_utxo_rehydrate.status"
# ts must be within 600s of KERNEL_DIFF_STATUS_MOCK_NOW (stale check uses same clock source).
printf '%s\n' '50000000 82581490 581543 2999950' >"$CHUNK_UTXO_REHYDRATE_STATUS_FILE"
printf '%s\n' '   [disk-utxo] commit 61%  +2000000 rows in 8.0s (250 k/s)  total 50000000/82581490' >"$CHUNK_UTXO_CHECKPOINT_LOG"
export KERNEL_DIFF_STATUS_MOCK_NOW=3000000
out5="$("$STATUS" --once checkpoints 2>&1)" || fail "rehydr first poll"
echo "$out5" | grep -qE 'bps=~(250k/s|250\.0k/s)' || fail "expected bps=~250k/s from log batch rate, got: $out5"

printf '%s\n' '50800000 82581490 581543 3000001' >"$CHUNK_UTXO_REHYDRATE_STATUS_FILE"
printf '%s\n' '   [disk-utxo] commit 62%  +2000000 rows in 5.0s (400 k/s)  total 50800000/82581490' >>"$CHUNK_UTXO_CHECKPOINT_LOG"
export KERNEL_DIFF_STATUS_MOCK_NOW=3000002
out6="$("$STATUS" --once checkpoints 2>&1)" || fail "rehydr second poll"
# Window: (50800000 - 50000000) / (3000002 - 3000000) = 400k rows/s → 400.0k/s
echo "$out6" | grep -qE 'bps=400\.0k/s' || fail "expected windowed bps=400.0k/s, got: $out6"

echo "OK: kernel-diff-status BPS tests passed (mocked clock + persisted poll state + rehydr rows/s)."
