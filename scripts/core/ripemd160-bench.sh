#!/bin/bash
# Bitcoin Core RIPEMD160 Benchmark (Portable)

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="$RESULTS_DIR_FALLBACK"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/ripemd160-bench-$(date +%Y%m%d-%H%M%S).json"

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
OUTPUT_FILE="$OUTPUT_DIR/core-ripemd160-bench-$(date +%Y%m%d-%H%M%S).json"

# Bitcoin Core RIPEMD160 Benchmark
# Measures RIPEMD160 hash performance using bench_bitcoin



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

echo "Running bench_bitcoin for RIPEMD160 operations (this may take a few minutes)..."
echo "This benchmarks RIPEMD160 hash performance."

# Run bench_bitcoin and capture output
BENCH_OUTPUT=$("$BENCH_BITCOIN" -filter="RIPEMD160" 2>&1 || echo "")

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

RIPEMD160_LINE=$(echo "$BENCH_OUTPUT" | grep -iE "RIPEMD160|BenchRIPEMD160" | head -1 || echo "")

RIPEMD160_DATA=$(parse_bench_bitcoin "$RIPEMD160_LINE")
RIPEMD160_TIME_NS=$(echo "$RIPEMD160_DATA" | cut -d'|' -f1)
RIPEMD160_OPS=$(echo "$RIPEMD160_DATA" | cut -d'|' -f2)

BENCHMARKS="[]"

if [ "$RIPEMD160_TIME_NS" != "0" ]; then
    TIME_MS=$(awk "BEGIN {printf \"%.6f\", $RIPEMD160_TIME_NS / 1000000}")
    # Use direct number substitution (no --argjson needed)
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "BenchRIPEMD160" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $RIPEMD160_TIME_NS, \"ops_per_sec\": $RIPEMD160_OPS}]" 2>/dev/null || echo "$BENCHMARKS")
fi

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "bench_bitcoin (RIPEMD160 hash operations)",
  "benchmarks": $BENCHMARKS
}
EOF
echo "✅ Results saved to: $OUTPUT_FILE"


