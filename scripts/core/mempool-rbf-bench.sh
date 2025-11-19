#!/bin/bash
# Bitcoin Core Mempool RBF (Replace-By-Fee) Benchmark (Portable)

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="$RESULTS_DIR_FALLBACK"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/mempool-rbf-bench-$(date +%Y%m%d-%H%M%S).json"

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
OUTPUT_FILE="$OUTPUT_DIR/core-mempool-rbf-bench-$(date +%Y%m%d-%H%M%S).json"

# Bitcoin Core Mempool RBF (Replace-By-Fee) Benchmark
# Measures RBF checks using bench_bitcoin



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

echo "Running RBF benchmarks..."
echo "Note: Core may not have a specific RBF benchmark."
echo "Using MempoolCheck and MempoolEviction as proxies for mempool operations including RBF."

LOG_FILE="/tmp/core-mempool-rbf.log"
# Run all benchmarks and extract mempool-related ones
if "$BENCH_BITCOIN" 2>&1 | tee "$LOG_FILE"; then
    BENCHMARKS="[]"
    # Extract MempoolCheck and MempoolEviction (use MempoolCheck as primary for RBF)
    while IFS= read -r line; do
        if echo "$line" | grep -qE '`MempoolCheck`|`MempoolEviction`' && ! echo "$line" | grep -q "EphemeralSpends"; then
            BENCH_NAME=$(echo "$line" | grep -oE '`[^`]+`' | tr -d '`' | head -1 || echo "")
            TIME_NS=$(echo "$line" | awk -F'|' '{gsub(/[^0-9.]/,"",$2); print $2}' 2>/dev/null | head -1 || echo "")
            
            if [ -n "$BENCH_NAME" ] && [ -n "$TIME_NS" ] && [ "$TIME_NS" != "0" ] && [ "$TIME_NS" != "" ]; then
                TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}")
                BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$BENCH_NAME" --arg time "$TIME_MS" --arg timens "$TIME_NS" '. += [{"name": $name, "time_ms": ($time | tonumber), "time_ns": ($timens | tonumber)}]' 2>/dev/null || echo "$BENCHMARKS")
            fi
        fi
    done < "$LOG_FILE"
else
    BENCHMARKS="[]"
fi

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "bench_bitcoin (mempool operations - proxy for RBF)",
  "benchmark_suite": "mempool_rbf",
  "benchmarks": $BENCHMARKS,
  "note": "Core may not have a specific RBF benchmark. Using MempoolCheck/MempoolEviction as proxies."
}
EOF

echo "✅ Results saved to: $OUTPUT_FILE"

