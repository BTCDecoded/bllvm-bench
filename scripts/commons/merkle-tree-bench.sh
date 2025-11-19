#!/bin/bash
# Bitcoin Commons Merkle Tree Operations Benchmark
# Measures merkle root calculation using Criterion

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/commons-merkle-tree-bench-$(date +%Y%m%d-%H%M%S).json"
BENCH_DIR="$BLLVM_BENCH_ROOT"

echo "=== Bitcoin Commons Merkle Tree Operations Benchmark ==="
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

echo "Running merkle tree benchmarks..."
LOG_FILE="/tmp/commons-merkle.log"
RUSTFLAGS="-C target-cpu=native" cargo bench --bench hash_operations --features production 2>&1 | tee "$LOG_FILE" || true

# Extract from Criterion JSON files (more reliable than parsing stdout)
CRITERION_DIR="$BENCH_DIR/target/criterion"
BENCHMARKS="[]"

# Look for merkle root benchmarks
for bench_dir in "$CRITERION_DIR"/merkle_root* "$CRITERION_DIR"/calculate_merkle_root*; do
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

# If no benchmarks found from JSON, try old pattern
if [ "$BENCHMARKS" = "[]" ]; then
    # Extract all merkle_root benchmarks
    echo "$BENCH_OUTPUT" | grep -A 5 "merkle_root" | while IFS= read -r line; do
        if echo "$line" | grep -qE "merkle_root_[0-9]+tx"; then
            BENCH_NAME=$(echo "$line" | grep -oE "merkle_root_[0-9]+tx" | head -1)
            # Get the time line that follows
            TIME_LINE=$(echo "$BENCH_OUTPUT" | grep -A 3 "$BENCH_NAME" | grep "time:" | head -1)
            if [ -n "$TIME_LINE" ]; then
                # Extract median time (middle value): [min ns median ns max ns]
                TIME_NS=$(echo "$TIME_LINE" | sed -n 's/.*\[\([0-9.]*\)[^0-9]*\([0-9.]*\)[^0-9]*\([0-9.]*\)\].*/\2/p' | head -1)
                if [ -n "$TIME_NS" ] && [ -n "$BENCH_NAME" ]; then
                    TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
                    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$BENCH_NAME" --arg name "$name" ". += [{"name": $name, "time_ms": $TIME_MS, "time_ns": $TIME_NS}]" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns}]' 2>/dev/null || echo "$BENCHMARKS")
                fi
            fi
        fi
    done
    # Already tried JSON above, nothing more to do
fi

if [ "$BENCHMARKS" != "[]" ]; then
    
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmarks (merkle root calculation)",
  "benchmark_suite": "merkle_tree",
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
