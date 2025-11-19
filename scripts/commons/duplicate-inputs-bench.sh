#!/bin/bash
# Bitcoin Commons Duplicate Input Detection Benchmark
# Measures duplicate input detection using Criterion

set -e

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
# Convert to absolute path
OUTPUT_DIR="$(cd "$OUTPUT_DIR" 2>/dev/null && pwd || echo "$(cd "$(dirname "$0")/.." && pwd)/results")"
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/commons-duplicate-inputs-bench-$(date +%Y%m%d-%H%M%S).json"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"
BENCH_DIR="$BLLVM_BENCH_ROOT"

echo "=== Bitcoin Commons Duplicate Input Detection Benchmark ==="
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

echo "Running duplicate input detection benchmarks..."
LOG_FILE="/tmp/commons-duplicate.log"
BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench hash_operations --features production 2>&1 | tee "$LOG_FILE" || echo "")

BENCHMARKS="[]"

# Parse Criterion output - look for duplicate_inputs benchmarks
# New benchmark: duplicate_inputs_block_validation (validates entire block like Core)
# Old benchmarks: duplicate_inputs_Ninputs_Nduplicates (single transaction only)
if echo "$BENCH_OUTPUT" | grep -q "duplicate_inputs"; then
    # First, try to find the new block validation benchmark (matches Core's CheckBlock)
    if echo "$BENCH_OUTPUT" | grep -q "duplicate_inputs_block_validation"; then
        BENCH_NAME="duplicate_inputs_block_validation"
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
    
    # Also extract old benchmarks for backward compatibility
    echo "$BENCH_OUTPUT" | grep -A 5 "duplicate_inputs" | while IFS= read -r line; do
        if echo "$line" | grep -qE "duplicate_inputs_[0-9]+inputs"; then
            BENCH_NAME=$(echo "$line" | grep -oE "duplicate_inputs_[0-9]+inputs_[0-9]+duplicates" | head -1)
            # Skip if we already have the block validation benchmark
            if [ "$BENCH_NAME" != "duplicate_inputs_block_validation" ]; then
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
        fi
    done
fi

# If no benchmarks found from stdout, try parsing from Criterion's JSON output
if [ "$BENCHMARKS" = "[]" ]; then
    CRITERION_DIR="$BENCH_DIR/target/criterion"
    if [ -d "$CRITERION_DIR" ]; then
        # Prioritize block validation benchmark (matches Core's CheckBlock)
        for bench_dir in "$CRITERION_DIR"/duplicate_inputs_block_validation*; do
            if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
                BENCH_NAME=$(basename "$bench_dir")
                TIME_NS=$(jq -r '.mean.point_estimate' "$bench_dir/base/estimates.json" 2>/dev/null || echo "")
                if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
                    TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
                    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$BENCH_NAME" --arg name "$name" ". += [{"name": $name, "time_ms": $TIME_MS, "time_ns": $TIME_NS}]" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns}]' 2>/dev/null || echo "$BENCHMARKS")
                fi
            fi
        done
        
        # Also check for old single-transaction benchmarks
        for bench_dir in "$CRITERION_DIR"/duplicate_inputs_*inputs_*duplicates*; do
            if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
                BENCH_NAME=$(basename "$bench_dir")
                # Skip if we already have block validation
                if echo "$BENCHMARKS" | jq -e --arg name "$BENCH_NAME" '.benchmarks[] | select(.name == $name)' > /dev/null 2>&1; then
                    continue
                fi
                TIME_NS=$(jq -r '.mean.point_estimate' "$bench_dir/base/estimates.json" 2>/dev/null || echo "")
                if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
                    TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
                    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$BENCH_NAME" --arg name "$name" ". += [{"name": $name, "time_ms": $TIME_MS, "time_ns": $TIME_NS}]" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns}]' 2>/dev/null || echo "$BENCHMARKS")
                fi
            fi
        done
    fi
fi

if [ "$BENCHMARKS" != "[]" ]; then
    
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmarks (duplicate input detection)",
  "benchmark_suite": "duplicate_inputs",
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
