#!/bin/bash
# Bitcoin Commons Mempool Acceptance Benchmark
# Measures mempool acceptance operations using Criterion

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/commons-mempool-acceptance-bench-$(date +%Y%m%d-%H%M%S).json"
BENCH_DIR="$BLLVM_BENCH_ROOT"

echo "=== Bitcoin Commons Mempool Acceptance Benchmark ==="
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

echo "Running mempool acceptance benchmarks..."
LOG_FILE="/tmp/commons-mempool-acceptance.log"
if cargo bench --bench mempool_operations --features production 2>&1 | tee "$LOG_FILE"; then
    BENCHMARKS="[]"
    # Extract from Criterion JSON output (more reliable)
    CRITERION_DIR="$BENCH_DIR/target/criterion"
    # Look for accept_to_memory_pool_complex (validates 400 transactions like Core's MempoolCheck)
    bench_dir="$CRITERION_DIR/accept_to_memory_pool_complex"
    if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
        TIME_NS=$(jq -r '.mean.point_estimate' "$bench_dir/base/estimates.json" 2>/dev/null || echo "")
        if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            # Output both names for compatibility
            # Use direct number substitution (no --argjson needed)
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "accept_to_memory_pool_complex" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $TIME_NS}]" 2>/dev/null || echo "$BENCHMARKS")
            # Also add alias for report generator
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "accept_to_memory_pool_400tx" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $TIME_NS}]" 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
    
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmarks (accept_to_memory_pool)",
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
