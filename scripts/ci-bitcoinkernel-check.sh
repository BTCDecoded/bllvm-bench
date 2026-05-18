#!/usr/bin/env bash
# Optional CI / local check: compile block_kernel_diff with libbitcoinkernel.
# Plan §1d: default PR CI does not require Core; run this when BITCOIN_CORE_LIB_DIR is set.
#
# Usage:
#   export BITCOIN_CORE_LIB_DIR=/path/to/dir/with/libbitcoinkernel.so
#   ./scripts/ci-bitcoinkernel-check.sh

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ -z "${BITCOIN_CORE_LIB_DIR:-}" ]]; then
  echo "Skip: BITCOIN_CORE_LIB_DIR not set (libbitcoinkernel link not attempted)."
  exit 0
fi

echo "BITCOIN_CORE_LIB_DIR=$BITCOIN_CORE_LIB_DIR"
exec cargo check --features bitcoinkernel --bin block_kernel_diff
