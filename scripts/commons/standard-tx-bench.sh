#!/bin/bash
# Bitcoin Commons Standard Transaction Checks Benchmark
# Measures is_standard_tx checks using Criterion

set -e

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
# OUTPUT_DIR already set by get_output_dir
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/commons-standard-tx-bench-$(date +%Y%m%d-%H%M%S).json"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"
BENCH_DIR="$BLLVM_BENCH_ROOT"

echo "=== Bitcoin Commons Standard Transaction Checks Benchmark ==="
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

echo "Running standard transaction checks benchmarks..."
LOG_FILE="/tmp/commons-standard-tx.log"
if cargo bench --bench mempool_operations --features production 2>&1 | tee "$LOG_FILE"; then
    BENCHMARKS="[]"
    # Extract from Criterion JSON output (more reliable)
    CRITERION_DIR="$BENCH_DIR/target/criterion"
    bench_dir="$CRITERION_DIR/is_standard_tx_400tx"
    if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
        TIME_NS=$(jq -r '.mean.point_estimate' "$bench_dir/base/estimates.json" 2>/dev/null || echo "")
        if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "is_standard_tx_400tx" --arg name "$name" ". += [{"name": $name, "time_ms": $TIME_MS, "time_ns": $TIME_NS}]" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns}]' 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
    
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmarks (is_standard_tx)",
  "benchmark_suite": "mempool_operations",
  "benchmarks": $BENCHMARKS,
  "log_file": "$LOG_FILE"
}
EOF
    echo "✅ Results saved to: $OUTPUT_FILE"
else
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Benchmark execution failed"
}
EOF
    exit 1
fi
