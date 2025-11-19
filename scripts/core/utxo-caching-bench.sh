#!/bin/bash
# Bitcoin Core UTXO Caching Benchmark
# Measures CCoinsCaching performance using bench_bitcoin
# Fair comparison with Commons UTXO caching

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="$RESULTS_DIR_FALLBACK"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/utxo-caching-bench-$(date +%Y%m%d-%H%M%S).json"

# Set trap to ensure JSON is always written, even on unexpected exit
trap 'if [ -n "$OUTPUT_FILE" ] && [ ! -f "$OUTPUT_FILE" ]; then echo "{\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"error\":\"Script exited unexpectedly before writing JSON\",\"script\":\"$0\"}" > "$OUTPUT_FILE" 2>/dev/null || true; fi' EXIT ERR

source "$SCRIPT_DIR/../shared/common.sh" || {
    echo "❌ Failed to source common.sh"
    exit 1
}

# Verify get_bench_bitcoin function is available
if ! type get_bench_bitcoin >/dev/null 2>&1; then
        cat > "$OUTPUT_FILE" << EOF
    {
      "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
      "error": "get_bench_bitcoin function not found",
      "script": "$0"
    }
    EOF
        exit 0
fi

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/core-utxo-caching-bench-$(date +%Y%m%d-%H%M%S).json"

# Reliably find or build bench_bitcoin
BENCH_BITCOIN=$(get_bench_bitcoin)

if [ -z "$BENCH_BITCOIN" ] || [ ! -f "$BENCH_BITCOIN" ]; then
    echo "❌ bench_bitcoin not found or not executable"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bench_bitcoin not found",
  "core_path": "${CORE_PATH:-not_set}",
  "note": "Please build Core with: cd \$CORE_PATH && cmake -B build -DBUILD_BENCH=ON && cmake --build build -t bench_bitcoin"
}
EOF
    echo "✅ Error JSON written to: $OUTPUT_FILE"
    exit 0
fi

echo "Running bench_bitcoin for UTXO operations (this may take 1-2 minutes)..."
echo "This benchmarks UTXO insert/get/remove operations (matches Commons' utxo_insert/utxo_get/utxo_remove)."

# Run bench_bitcoin and capture output for all UTXO operations
# Run all three benchmarks separately and combine output
BENCH_OUTPUT=$("$BENCH_BITCOIN" -filter="UTXOInsert|UTXOGet|UTXORemove" 2>&1 || echo "")

# Parse bench_bitcoin output
# Format: "| 163.32 | 6,123,018.76 | 1.1% | 1,763.00 | 598.70 | 2.945 | 93.00 | 0.0% | 0.01 | `UTXOInsert`"
# Column 1: median time in ns, Column 2: ops/sec
parse_bench_bitcoin() {
    local line="$1"
    if [ -z "$line" ]; then
        echo "0|0"
        return
    fi
    # Extract median time (first column after the leading |)
    # Remove commas and extract the number
    time_ns=$(echo "$line" | awk -F'|' '{gsub(/[^0-9.]/,"",$2); print $2}' 2>/dev/null || echo "0")
    # Extract ops/sec (second column after the leading |)
    ops_per_sec=$(echo "$line" | awk -F'|' '{gsub(/[^0-9.]/,"",$3); print $3}' 2>/dev/null || echo "0")
    echo "${time_ns}|${ops_per_sec}"
}

UTXO_INSERT_LINE=$(echo "$BENCH_OUTPUT" | grep -iE "UTXOInsert" | head -1 || echo "")
UTXO_GET_LINE=$(echo "$BENCH_OUTPUT" | grep -iE "UTXOGet" | head -1 || echo "")
UTXO_REMOVE_LINE=$(echo "$BENCH_OUTPUT" | grep -iE "UTXORemove" | head -1 || echo "")

UTXO_INSERT_DATA=$(parse_bench_bitcoin "$UTXO_INSERT_LINE")
UTXO_GET_DATA=$(parse_bench_bitcoin "$UTXO_GET_LINE")
UTXO_REMOVE_DATA=$(parse_bench_bitcoin "$UTXO_REMOVE_LINE")

UTXO_INSERT_TIME_NS=$(echo "$UTXO_INSERT_DATA" | cut -d'|' -f1)
UTXO_INSERT_OPS=$(echo "$UTXO_INSERT_DATA" | cut -d'|' -f2)
UTXO_GET_TIME_NS=$(echo "$UTXO_GET_DATA" | cut -d'|' -f1)
UTXO_GET_OPS=$(echo "$UTXO_GET_DATA" | cut -d'|' -f2)
UTXO_REMOVE_TIME_NS=$(echo "$UTXO_REMOVE_DATA" | cut -d'|' -f1)
UTXO_REMOVE_OPS=$(echo "$UTXO_REMOVE_DATA" | cut -d'|' -f2)

BENCHMARKS="[]"

if [ "$UTXO_INSERT_TIME_NS" != "0" ] && [ -n "$UTXO_INSERT_TIME_NS" ]; then
    TIME_MS=$(awk "BEGIN {printf \"%.6f\", $UTXO_INSERT_TIME_NS / 1000000}" 2>/dev/null || echo "0")
    # Use direct number substitution (no --argjson needed)
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "UTXOInsert" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $UTXO_INSERT_TIME_NS, \"ops_per_sec\": $UTXO_INSERT_OPS}]" 2>/dev/null || echo "$BENCHMARKS")
fi

if [ "$UTXO_GET_TIME_NS" != "0" ] && [ -n "$UTXO_GET_TIME_NS" ]; then
    TIME_MS=$(awk "BEGIN {printf \"%.6f\", $UTXO_GET_TIME_NS / 1000000}" 2>/dev/null || echo "0")
    # Use direct number substitution (no --argjson needed)
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "UTXOGet" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $UTXO_GET_TIME_NS, \"ops_per_sec\": $UTXO_GET_OPS}]" 2>/dev/null || echo "$BENCHMARKS")
fi

if [ "$UTXO_REMOVE_TIME_NS" != "0" ] && [ -n "$UTXO_REMOVE_TIME_NS" ]; then
    TIME_MS=$(awk "BEGIN {printf \"%.6f\", $UTXO_REMOVE_TIME_NS / 1000000}" 2>/dev/null || echo "0")
    # Use direct number substitution (no --argjson needed)
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "UTXORemove" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $UTXO_REMOVE_TIME_NS, \"ops_per_sec\": $UTXO_REMOVE_OPS}]" 2>/dev/null || echo "$BENCHMARKS")
fi

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "bench_bitcoin (UTXO operations - insert/get/remove matching Commons)",
  "benchmarks": $BENCHMARKS
}
EOF

echo "✅ Results saved to: $OUTPUT_FILE"

