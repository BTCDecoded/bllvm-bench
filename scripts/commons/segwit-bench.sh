#!/bin/bash
# Bitcoin Commons SegWit Operations Benchmark
# Measures SegWit operation performance using Criterion

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/commons-segwit-bench-$(date +%Y%m%d-%H%M%S).json"
BENCH_DIR="$BLLVM_BENCH_ROOT"

echo "=== Bitcoin Commons SegWit Operations Benchmark ==="
echo ""

if [ ! -d "$BENCH_DIR" ]; then
    echo "❌ bllvm-bench directory not found at $BENCH_DIR"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bllvm-bench directory not found",
  "note": "Check Commons installation"
}
EOF
    exit 1
fi

cd "$BENCH_DIR"

echo "Running SegWit operations benchmarks..."
echo "This measures SegWit transaction and block weight calculations"
echo ""

# Run Criterion benchmarks and parse output
LOG_FILE="/tmp/commons-segwit.log"
cargo bench --bench segwit_operations --features production 2>&1 | tee "$LOG_FILE" || true

# Extract from Criterion JSON files (more reliable than parsing stdout)
CRITERION_DIR="$BENCH_DIR/target/criterion"
BENCHMARKS="[]"

# Look for SegWit operation benchmarks
for bench_dir in "$CRITERION_DIR"/is_segwit_transaction* "$CRITERION_DIR"/calculate_transaction_weight* "$CRITERION_DIR"/calculate_block_weight* "$CRITERION_DIR"/segwit_*; do
    if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
        BENCH_NAME=$(basename "$bench_dir")
        TIME_NS=$(jq -r '.mean.point_estimate' "$bench_dir/base/estimates.json" 2>/dev/null || echo "")
        if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            TIME_NS_INT=$(awk "BEGIN {printf \"%.0f\", $TIME_NS}" 2>/dev/null || echo "0")
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$BENCH_NAME" --arg name "$name" ". += [{"name": $name, "time_ms": $TIME_MS, "time_ns": $TIME_NS_INT}]" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns}]' 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
done

# Generate JSON output
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "bitcoin_commons_segwit_operations": {
    "benchmarks": $BENCHMARKS,
    "measurement_method": "Criterion - Commons' actual SegWit implementation",
    "comparison_note": "This measures actual SegWit operations - comparable to Core's ConnectBlockAllEcdsa/AllSchnorr benchmarks"
  }
}
EOF

echo "Results saved to: $OUTPUT_FILE"
echo "$BENCHMARKS" | jq -r '.[] | "\(.name): \(.time_ms) ms"' 2>/dev/null || echo "Benchmarks completed"
