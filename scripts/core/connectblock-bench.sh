#!/bin/bash
# Bitcoin Core ConnectBlock Benchmark (Portable)

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="$RESULTS_DIR_FALLBACK"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/connectblock-bench-$(date +%Y%m%d-%H%M%S).json"

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

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/core-connectblock-bench-$(date +%Y%m%d-%H%M%S).json"

# Bitcoin Core ConnectBlock Benchmark
# Uses existing block-validation-bench.sh but with different output name
# This is a duplicate/alias for the ConnectBlock benchmark

echo "=== Bitcoin Core ConnectBlock Benchmark ==="
echo ""

# Use the existing block validation benchmark script
"$SCRIPT_DIR/../core/block-validation-bench.sh" "$OUTPUT_DIR"

# Find the most recent core-block-validation-bench file and copy/rename it
LATEST_BLOCK_VAL=$(find "$OUTPUT_DIR" -name "core-block-validation-bench-*.json" -type f 2>/dev/null | xargs ls -t 2>/dev/null | head -1)

if [ -n "$LATEST_BLOCK_VAL" ] && [ -f "$LATEST_BLOCK_VAL" ]; then
    # Extract the primary comparison time from the block validation JSON
    PRIMARY_TIME_MS=$(jq -r '.bitcoin_core_block_validation.primary_comparison.time_per_block_ms // .bitcoin_core_block_validation.connect_block_mixed_ecdsa_schnorr.time_per_block_ms // empty' "$LATEST_BLOCK_VAL" 2>/dev/null || echo "")
    PRIMARY_TIME_NS=$(jq -r '.bitcoin_core_block_validation.primary_comparison.time_per_block_ns // .bitcoin_core_block_validation.connect_block_mixed_ecdsa_schnorr.time_per_block_ns // empty' "$LATEST_BLOCK_VAL" 2>/dev/null || echo "")
    
    if [ -n "$PRIMARY_TIME_MS" ] && [ "$PRIMARY_TIME_MS" != "null" ] && [ "$PRIMARY_TIME_MS" != "" ]; then
        cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "bench_bitcoin (ConnectBlock with 1000 transactions)",
  "benchmark_suite": "connectblock",
  "benchmarks": [
    {
      "name": "ConnectBlockMixedEcdsaSchnorr",
      "time_ms": $PRIMARY_TIME_MS,
      "time_ns": $PRIMARY_TIME_NS
    }
  ],
  "note": "Primary comparison metric (mixed ECDSA/Schnorr, most realistic)"
}
EOF
        echo "✅ Results saved to: $OUTPUT_FILE"
    else
        echo "⚠️  Could not extract ConnectBlock timing from block validation results"
        cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Could not extract ConnectBlock timing from block validation results",
  "source_file": "$LATEST_BLOCK_VAL"
}
EOF
    fi
else
    echo "⚠️  Block validation benchmark file not found"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Block validation benchmark file not found"
}
EOF
fi

