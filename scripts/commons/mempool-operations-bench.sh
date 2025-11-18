#!/bin/bash
# Bitcoin Commons Mempool Operations Benchmark
# Measures mempool operations using Criterion

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/commons-mempool-operations-bench-$(date +%Y%m%d-%H%M%S).json"
BENCH_DIR="$BLLVM_BENCH_ROOT"

echo "=== Bitcoin Commons Mempool Operations Benchmark ==="
echo ""

if [ ! -d "$BENCH_DIR" ]; then
    echo "❌ bllvm-bench directory not found"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bllvm-bench directory not found"
}
EOF
    exit 1
fi

cd "$BENCH_DIR"

echo "Running mempool operations benchmarks..."
LOG_FILE="/tmp/commons-mempool.log"
BENCH_SUCCESS=false

if cargo bench --bench mempool_operations --features production 2>&1 | tee "$LOG_FILE"; then
    BENCH_SUCCESS=true
    echo "✅ mempool_operations benchmark completed"
else
    echo "❌ mempool_operations benchmark failed - check $LOG_FILE"
fi

# Extract from Criterion JSON files (more reliable than parsing stdout)
CRITERION_DIR="$BENCH_DIR/target/criterion"
BENCHMARKS="[]"

# Verify Criterion output exists
if [ "$BENCH_SUCCESS" = "true" ] && [ ! -d "$CRITERION_DIR" ]; then
    echo "⚠️  WARNING: Criterion directory does not exist: $CRITERION_DIR"
    BENCH_SUCCESS=false
fi

if [ "$BENCH_SUCCESS" = "false" ]; then
    echo "⚠️  Benchmark failed - outputting error JSON"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Benchmark execution failed",
  "log_file": "$LOG_FILE",
  "measurement_method": "Criterion benchmarks (actual mempool operations)",
  "benchmark_suite": "mempool_operations",
  "benchmarks": [],
  "note": "Check log file for details"
}
EOF
    exit 0
fi

# Look for mempool operation benchmarks
for bench_dir in "$CRITERION_DIR"/accept_to_memory_pool* "$CRITERION_DIR"/is_standard_tx* "$CRITERION_DIR"/replacement_checks* "$CRITERION_DIR"/mempool_*; do
    if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
        BENCH_NAME=$(basename "$bench_dir")
        TIME_NS=$(jq -r '.mean.point_estimate' "$bench_dir/base/estimates.json" 2>/dev/null || echo "")
        if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            TIME_NS_INT=$(awk "BEGIN {printf \"%.0f\", $TIME_NS}" 2>/dev/null || echo "0")
            
            # Extract statistical data
            STATS=$("$BLLVM_BENCH_ROOT/scripts/shared/extract-criterion-stats.sh" "$bench_dir/base/estimates.json")
            
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$BENCH_NAME" \
                --argjson time_ms "$TIME_MS" \
                --argjson time_ns "$TIME_NS_INT" \
                --argjson stats "$STATS" \
                '. += [{
                    "name": $name,
                    "time_ms": $time_ms,
                    "time_ns": $time_ns,
                    "statistics": $stats
                }]' 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
done

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmarks (actual mempool operations)",
  "benchmark_suite": "mempool_operations",
  "benchmarks": $BENCHMARKS,
  "log_file": "$LOG_FILE"
}
EOF
echo "✅ Results saved to: $OUTPUT_FILE"
