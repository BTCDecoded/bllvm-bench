#!/usr/bin/env bash
# Clean rehydrate + resume chunk_utxo_checkpoints, with optional LAN/Start9 chunk repair via RPC.
#
# Loads env from BLVM_BENCH_ENV_FILE (default: blvm-bench/.env) so REMOTE_CORE_* / START9_* /
# LAND_NODE_* match remote_core_rpc.rs (SSH + nsenter to bitcoind on the LAN node).
#
# Usage:
#   export BLOCK_CACHE_DIR=/path/to/blockchain
#   ./scripts/chunk-utxo-resume-with-fallback.sh resume-clean 680000
#
# Optional: after checkpoint failure, rebuild chunks from the node (slow; full chunk pass):
#   CHUNK_UTXO_FALLBACK_COLLECT=1 ./scripts/chunk-utxo-resume-with-fallback.sh resume-with-fallback 680000
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SEG="$ROOT/scripts/chunk-utxo-checkpoint-segments.sh"
COLLECT_BIN="${COLLECT_CHUNKS_RPC_BIN:-$ROOT/target/release/collect_chunks_rpc}"
ENV_FILE="${BLVM_BENCH_ENV_FILE:-$ROOT/.env}"

load_env() {
  if [[ -f "$ENV_FILE" ]]; then
    set -a
    # shellcheck disable=SC1090
    source "$ENV_FILE"
    set +a
    echo "   [chunk-utxo-fallback] loaded $ENV_FILE" >&2
  else
    echo "   [chunk-utxo-fallback] no env file at $ENV_FILE (set BLVM_BENCH_ENV_FILE or create .env)" >&2
  fi
}

remote_core_configured() {
  local k h u p
  k="${REMOTE_CORE_SSH_KEY:-${LAND_NODE_SSH_KEY:-${START9_SSH_KEY:-}}}"
  h="${REMOTE_CORE_SSH_HOST:-${LAND_NODE_SSH_HOST:-${START9_SSH_HOST:-}}}"
  u="${REMOTE_CORE_RPC_USER:-${LAND_NODE_RPC_USER:-${START9_RPC_USER:-}}}"
  p="${REMOTE_CORE_RPC_PASSWORD:-${LAND_NODE_RPC_PASSWORD:-${START9_RPC_PASSWORD:-}}}"
  [[ -n "$k" && -n "$h" && -n "$u" && -n "$p" ]]
}

wipe_rocksdb() {
  local db="${DISK_UTXO_DB_PATH:-${XDG_CACHE_HOME:-$HOME/.cache}/blvm-bench/utxo_rocksdb}"
  echo "   [chunk-utxo-fallback] rm -rf $db" >&2
  rm -rf "$db"
}

run_resume() {
  exec "$SEG" resume-from "$@"
}

try_resume() {
  "$SEG" resume-from "$@"
}

usage() {
  sed -n '2,18p' "$0" | sed 's/^# \?//'
  exit "${1:-0}"
}

case "${1:-}" in
  help|-h|--help|"")
    usage 0
    ;;
  resume-clean)
    H="${2:?need checkpoint height H (loads utxo_H.bin)}"
    shift 2
    load_env
    wipe_rocksdb
    run_resume "$H" "$@"
    ;;
  resume-with-fallback)
    H="${2:?need checkpoint height H}"
    shift 2
    load_env
    wipe_rocksdb
    set +e
    try_resume "$H" "$@"
    ec=$?
    set -e
    if [[ "$ec" -ne 0 ]]; then
      echo "   [chunk-utxo-fallback] chunk_utxo resume exited $ec" >&2
      if [[ "${CHUNK_UTXO_FALLBACK_COLLECT:-0}" == "1" ]] && remote_core_configured; then
        if [[ ! -x "$COLLECT_BIN" ]]; then
          echo "   [chunk-utxo-fallback] missing $COLLECT_BIN — build: cargo build --release --bin collect_chunks_rpc" >&2
          exit "$ec"
        fi
        echo "   [chunk-utxo-fallback] CHUNK_UTXO_FALLBACK_COLLECT=1 — running collect_chunks_rpc (LAN/Start9 via REMOTE_CORE_*/START9_*; may take a long time)" >&2
        "$COLLECT_BIN" || {
          echo "   [chunk-utxo-fallback] collect_chunks_rpc failed" >&2
          exit "$ec"
        }
        wipe_rocksdb
        try_resume "$H" "$@"
        ec=$?
      elif remote_core_configured; then
        echo "   [chunk-utxo-fallback] To auto-repair chunks from the LAN node after failures, set CHUNK_UTXO_FALLBACK_COLLECT=1 and ensure collect_chunks_rpc is built." >&2
      else
        echo "   [chunk-utxo-fallback] For RPC chunk collection, set REMOTE_CORE_SSH_KEY, REMOTE_CORE_SSH_HOST, REMOTE_CORE_RPC_USER, REMOTE_CORE_RPC_PASSWORD (or legacy START9_* / LAND_NODE_*)." >&2
      fi
    fi
    exit "$ec"
    ;;
  *)
    echo "Unknown: $1" >&2
    usage 1
    ;;
esac
