#!/bin/bash
# Bitcoin Commons Transaction Validation Benchmark (Portable)
# Measures actual transaction validation performance using Criterion

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/commons-transaction-validation-bench-$(date +%Y%m%d-%H%M%S).json"

echo "=== Bitcoin Commons Transaction Validation Benchmark ==="
echo ""

BENCH_DIR="$BLLVM_BENCH_ROOT"

if [ ! -d "$BENCH_DIR" ]; then
    echo "❌ bllvm-bench directory not found at $BENCH_DIR"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bllvm-bench directory not found",
  "note": "This script should be run from within bllvm-bench"
}
EOF
    exit 1
fi

cd "$BENCH_DIR"

echo "Running transaction validation benchmarks..."
echo "NOTE: Core's DeserializeAndCheckBlockTest validates ENTIRE BLOCKS (deserialization + CheckBlock)"
echo "      To match Core, we use check_block benchmark (structure validation only, no scripts)"
echo "      This matches Core's CheckBlock operation exactly"
echo ""

LOG_FILE="/tmp/commons-tx-validation.log"
BENCH_SUCCESS=false

if cargo bench --bench check_block --features production 2>&1 | tee "$LOG_FILE"; then
    BENCH_SUCCESS=true
    echo "✅ check_block benchmark completed"
else
    echo "❌ check_block benchmark failed - check $LOG_FILE"
fi

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
  "measurement_method": "Criterion check_block benchmark (matches Core's DeserializeAndCheckBlockTest)",
  "benchmark_suite": "check_block",
  "benchmarks": [],
  "note": "Check log file for details"
}
EOF
    exit 0
fi

for bench_dir in "$CRITERION_DIR"/check_block*; do
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
  "measurement_method": "Criterion check_block benchmark (matches Core's DeserializeAndCheckBlockTest)",
  "benchmark_suite": "check_block",
  "comparison_note": "Core's DeserializeAndCheckBlockTest validates entire blocks (deserialization + CheckBlock). This benchmark uses Commons' check_block to match - validates block structure only (no script verification), exactly like Core's CheckBlock.",
  "benchmarks": $BENCHMARKS,
  "log_file": "$LOG_FILE"
}
EOF

echo "✅ Results saved to: $OUTPUT_FILE"
echo ""
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

