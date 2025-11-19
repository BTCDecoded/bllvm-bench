#!/bin/bash
# Bitcoin Core SegWit Operations Benchmark (Portable)

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="$RESULTS_DIR_FALLBACK"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/segwit-bench-$(date +%Y%m%d-%H%M%S).json"

# Set trap to ensure JSON is always written, even on unexpected exit
trap 'if [ -n "$OUTPUT_FILE" ] && [ ! -f "$OUTPUT_FILE" ]; then echo "{\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"error\":\"Script exited unexpectedly before writing JSON\",\"script\":\"$0\"}" > "$OUTPUT_FILE" 2>/dev/null || true; fi' EXIT ERR

source "$SCRIPT_DIR/../shared/common.sh" || {
    echo "❌ Failed to source common.sh" >&2
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Failed to source common.sh",
  "script": "$0"
}
EOF
    exit 0
}

# Verify get_bench_bitcoin function is available
if ! type get_bench_bitcoin >/dev/null 2>&1; then
if ! type get_bench_bitcoin >/dev/null 2>&1; then
    echo "❌ get_bench_bitcoin function not found after sourcing common.sh"
    exit 0
fi

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/core-segwit-bench-$(date +%Y%m%d-%H%M%S).json"

# Bitcoin Core SegWit Operations Benchmark
# Uses bench_bitcoin to benchmark SegWit block validation (ConnectBlockAllEcdsa/AllSchnorr)



# Reliably find or build bench_bitcoin
BENCH_BITCOIN=$(get_bench_bitcoin)

if [ -z "$BENCH_BITCOIN" ] || [ ! -f "$BENCH_BITCOIN" ]; then
    echo "❌ bench_bitcoin not found or not executable"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bench_bitcoin not found",
  "core_path": "${CORE_PATH:-not_set}",
  "note": "Please build Core with: cd $CORE_PATH && cmake -B build -DBUILD_BENCH=ON && cmake --build build -t bench_bitcoin"
}
EOF
    echo "✅ Error JSON written to: $OUTPUT_FILE"
    exit 0
fi

echo "Running bench_bitcoin for SegWit operations (this may take 1-2 minutes)..."
echo "This benchmarks SegWit block validation (ConnectBlockAllEcdsa/AllSchnorr)"

# Run bench_bitcoin and capture output
BENCH_OUTPUT=$("$BENCH_BITCOIN" 2>&1 || echo "")

# Extract SegWit-related benchmark results
CONNECT_BLOCK_ALL_SCHNORR=$(echo "$BENCH_OUTPUT" | grep -E "ConnectBlockAllSchnorr" | head -1 || echo "")
CONNECT_BLOCK_ALL_ECDSA=$(echo "$BENCH_OUTPUT" | grep -E "ConnectBlockAllEcdsa" | head -1 || echo "")
CONNECT_BLOCK_MIXED=$(echo "$BENCH_OUTPUT" | grep -E "ConnectBlockMixedEcdsaSchnorr" | head -1 || echo "")

# Parse bench_bitcoin output
parse_bench_bitcoin() {
    local line="$1"
    if [ -z "$line" ]; then
        echo "0|0"
        return
    fi
    time_ns=$(echo "$line" | awk -F'|' '{gsub(/[^0-9.]/,"",$2); print $2}' 2>/dev/null || echo "0")
    ops_per_sec=$(echo "$line" | awk -F'|' '{gsub(/[^0-9.]/,"",$3); print $3}' 2>/dev/null || echo "0")
    echo "${time_ns}|${ops_per_sec}"
}

ALL_SCHNORR_DATA=$(parse_bench_bitcoin "$CONNECT_BLOCK_ALL_SCHNORR")
ALL_SCHNORR_TIME_NS=$(echo "$ALL_SCHNORR_DATA" | cut -d'|' -f1)
ALL_SCHNORR_OPS=$(echo "$ALL_SCHNORR_DATA" | cut -d'|' -f2)

ALL_ECDSA_DATA=$(parse_bench_bitcoin "$CONNECT_BLOCK_ALL_ECDSA")
ALL_ECDSA_TIME_NS=$(echo "$ALL_ECDSA_DATA" | cut -d'|' -f1)
ALL_ECDSA_OPS=$(echo "$ALL_ECDSA_DATA" | cut -d'|' -f2)

MIXED_DATA=$(parse_bench_bitcoin "$CONNECT_BLOCK_MIXED")
MIXED_TIME_NS=$(echo "$MIXED_DATA" | cut -d'|' -f1)
MIXED_OPS=$(echo "$MIXED_DATA" | cut -d'|' -f2)

# Convert to milliseconds
ALL_SCHNORR_TIME_MS=$(awk "BEGIN {printf \"%.6f\", $ALL_SCHNORR_TIME_NS / 1000000}" 2>/dev/null || echo "0")
ALL_ECDSA_TIME_MS=$(awk "BEGIN {printf \"%.6f\", $ALL_ECDSA_TIME_NS / 1000000}" 2>/dev/null || echo "0")
MIXED_TIME_MS=$(awk "BEGIN {printf \"%.6f\", $MIXED_TIME_NS / 1000000}" 2>/dev/null || echo "0")

# Generate JSON output
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "bitcoin_core_segwit_operations": {
    "connect_block_all_schnorr": {
      "time_per_block_ns": $ALL_SCHNORR_TIME_NS,
      "time_per_block_ms": $ALL_SCHNORR_TIME_MS,
      "blocks_per_second": $ALL_SCHNORR_OPS,
      "implementation": "Chainstate::ConnectBlock (all Schnorr/Taproot signatures)",
      "note": "SegWit v1 (Taproot) block validation"
    },
    "connect_block_all_ecdsa": {
      "time_per_block_ns": $ALL_ECDSA_TIME_NS,
      "time_per_block_ms": $ALL_ECDSA_TIME_MS,
      "blocks_per_second": $ALL_ECDSA_OPS,
      "implementation": "Chainstate::ConnectBlock (all ECDSA/SegWit v0 signatures)",
      "note": "SegWit v0 block validation"
    },
    "connect_block_mixed": {
      "time_per_block_ns": $MIXED_TIME_NS,
      "time_per_block_ms": $MIXED_TIME_MS,
      "blocks_per_second": $MIXED_OPS,
      "implementation": "Chainstate::ConnectBlock (mixed ECDSA/Schnorr)",
      "note": "Mixed SegWit v0 and v1 block validation"
    },
    "primary_comparison": {
      "time_per_block_ms": $MIXED_TIME_MS,
      "time_per_block_ns": $MIXED_TIME_NS,
      "blocks_per_second": $MIXED_OPS,
      "note": "Primary metric for comparison (mixed, most realistic)"
    },
    "measurement_method": "bench_bitcoin - Core's actual ConnectBlock implementation with SegWit",
    "comparison_note": "This measures actual SegWit block validation - comparable to Commons' segwit_operations benchmark"
  }
}
EOF

echo "Results saved to: $OUTPUT_FILE"
echo "Primary (mixed): $MIXED_TIME_MS ms per block ($MIXED_OPS blocks/sec)"
echo "All Schnorr: $ALL_SCHNORR_TIME_MS ms per block ($ALL_SCHNORR_OPS blocks/sec)"
echo "All ECDSA: $ALL_ECDSA_TIME_MS ms per block ($ALL_ECDSA_OPS blocks/sec)"


