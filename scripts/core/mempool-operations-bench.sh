#!/bin/bash
# Bitcoin Core Mempool Operations Benchmark (Portable)

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="$RESULTS_DIR_FALLBACK"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/mempool-operations-bench-$(date +%Y%m%d-%H%M%S).json"

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
OUTPUT_FILE="$OUTPUT_DIR/core-mempool-operations-bench-$(date +%Y%m%d-%H%M%S).json"

# Bitcoin Core Mempool Operations Benchmark
# Measures mempool operations using bench_bitcoin

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

echo "Running mempool operations benchmarks..."
TEMP_JSON=$(mktemp)
LOG_FILE="/tmp/core-mempool.log"
# Run multiple mempool benchmarks and combine results
# Try multiple filter patterns to catch all mempool benchmarks
# Note: Core uses ComplexMemPool, not MempoolCheck/MempoolEviction
"$BENCH_BITCOIN" -filter="ComplexMemPool|MempoolCheck|MempoolCheckEphemeralSpends|MempoolEviction|RpcMempool" -min-time=500 2>&1 | tee "$LOG_FILE" || true

# Parse bench_bitcoin pipe-delimited table format
BENCHMARKS="[]"
while IFS= read -r line; do
    # Look for lines with backticks (benchmark names) or lines with mempool-related names
    if echo "$line" | grep -qE '`.*`|ComplexMemPool|MempoolCheck|MempoolEviction|MempoolAccept'; then
        # Try to extract benchmark name from backticks first
        BENCH_NAME=$(echo "$line" | grep -oE '`[^`]+`' | tr -d '`' | head -1 || echo "")
        # If no backticks, try to extract from the line directly
        if [ -z "$BENCH_NAME" ]; then
            BENCH_NAME=$(echo "$line" | grep -oE "(ComplexMemPool|MempoolCheck|MempoolEviction|MempoolAccept)" | head -1 || echo "")
        fi
        
        # Extract time value - try multiple column positions
        TIME_NS=$(echo "$line" | awk -F'|' '{for(i=2;i<=NF;i++){gsub(/[^0-9.]/,"",$i); if($i!="" && $i!="0"){print $i; exit}}}' 2>/dev/null | head -1 || echo "")
        # If that fails, try extracting any number from the line
        if [ -z "$TIME_NS" ] || [ "$TIME_NS" = "0" ]; then
            TIME_NS=$(echo "$line" | grep -oE '[0-9]+\.[0-9]+|[0-9]+' | head -1 || echo "")
        fi
        
        if [ -n "$BENCH_NAME" ] && [ -n "$TIME_NS" ] && [ "$TIME_NS" != "0" ] && [ "$TIME_NS" != "" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            # Use direct number substitution (no --argjson)
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$BENCH_NAME" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $TIME_NS}]" 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
done < "$LOG_FILE"

# Only write JSON if we found benchmarks
if [ "$BENCHMARKS" != "[]" ]; then
    
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "bench_bitcoin (actual mempool operations)",
  "benchmarks": $BENCHMARKS
}
EOF
else
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Benchmark execution failed"
}
EOF
    exit 1
fi

echo "✅ Results saved to: $OUTPUT_FILE"

