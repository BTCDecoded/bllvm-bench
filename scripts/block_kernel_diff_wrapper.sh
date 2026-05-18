#!/usr/bin/env bash
# Phase 1: invoke block_kernel_diff with conservative defaults when env vars are unset.
# Does not override explicit exports — only fills in blanks.
#
# Usage: block_kernel_diff_wrapper.sh [args passed to block_kernel_diff]
#   BLOCK_KERNEL_DIFF_BIN=path/to/block_kernel_diff (default: ../target/release/block_kernel_diff)

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
for _kf in "${KERNEL_DIFF_ENV_FILE:-}" \
           "$ROOT/kernel-diff.local.env" \
           "$ROOT/.env.local" \
           "$ROOT/.env"; do
  [[ -z "${_kf:-}" ]] && continue
  [[ -f "$_kf" ]] || continue
  set -a
  # shellcheck source=/dev/null
  source "$_kf"
  set +a
  break
done
BIN="${BLOCK_KERNEL_DIFF_BIN:-$ROOT/target/release/block_kernel_diff}"

if [[ ! -x "$BIN" && ! -f "$BIN" ]]; then
  echo "block_kernel_diff not found at $BIN — build with:" >&2
  echo "  cargo build --release --features bitcoinkernel --bin block_kernel_diff" >&2
  exit 1
fi

N=$(nproc 2>/dev/null || echo 4)
# Avoid oversubscription: cap Rayon pool on large machines unless user set RAYON_NUM_THREADS.
if [[ -z "${RAYON_NUM_THREADS:-}" ]]; then
  if (( N > 8 )); then
    export RAYON_NUM_THREADS=8
  else
    export RAYON_NUM_THREADS="$N"
  fi
fi

# BLVM script parallelism (production+rayon builds); keep modest default.
if [[ -z "${BLVM_SCRIPT_WORKERS:-}" ]]; then
  if (( N > 4 )); then
    export BLVM_SCRIPT_WORKERS=4
  else
    export BLVM_SCRIPT_WORKERS="$N"
  fi
fi

exec "$BIN" "$@"
