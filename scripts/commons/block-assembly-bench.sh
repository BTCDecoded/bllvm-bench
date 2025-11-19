#!/bin/bash
# Bitcoin Commons Block Assembly Benchmark
# Measures block assembly performance using Criterion
set -e

# Source common functions FIRST
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh" || {
    echo "❌ Failed to source common.sh" >&2
    # Define fallback function if sourcing failed
    get_output_dir() {
        local output_dir="${1:-$(pwd)/results}"
        mkdir -p "$output_dir" 2>/dev/null || true
        echo "$output_dir"
    }
}

# Verify get_output_dir is available
if ! type get_output_dir >/dev/null 2>&1; then
    echo "❌ get_output_dir function not found after sourcing common.sh" >&2
    exit 1
fi

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
# OUTPUT_DIR already set by get_output_dir # Ensure absolute path
mkdir -p "$OUTPUT_DIR"
BENCH_DIR="$BLLVM_BENCH_ROOT"
OUTPUT_FILE="$OUTPUT_DIR/commons-block-assembly-bench-$(date +%Y%m%d-%H%M%S).json"

echo "=== Bitcoin Commons Block Assembly Benchmark ==="
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

echo "Running Commons Block Assembly benchmark..."
echo "Output: $OUTPUT_FILE"

LOG_FILE="/tmp/commons-block-assembly.log"
# Run block_assembly benchmark
BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench block_assembly --features production -- --test-threads=1 2>&1 | tee "$LOG_FILE" || echo "")

BENCHMARKS="[]"
CURRENT_BENCH=""

# Parse block_assembly benchmarks from output
while IFS= read -r line; do
    # Extract benchmark name from "Benchmarking <name>:" line
    if echo "$line" | grep -qE "^Benchmarking.*block_assembly"; then
        CURRENT_BENCH=$(echo "$line" | sed 's/^Benchmarking //' | sed 's/:$//' | awk '{print $1}' | tr -d ' ')
    fi
    # Extract time from "time: [min median max]" line
    if echo "$line" | grep -qE "time:\s*\[" && echo "$CURRENT_BENCH" | grep -q "block_assembly"; then
        if [ -z "$CURRENT_BENCH" ]; then
            CURRENT_BENCH=$(echo "$line" | awk '{print $1}' | tr -d ' ' | tr -d ':')
        fi
        # Extract median time and unit
        TIME_VALUE=$(echo "$line" | sed -n 's/.*\[[0-9.]* [a-zµ]* \([0-9.]*\) \([a-zµ]*\) [0-9.]* [a-zµ]*\].*/\1/p' | head -1)
        TIME_UNIT=$(echo "$line" | sed -n 's/.*\[[0-9.]* [a-zµ]* [0-9.]* \([a-zµ]*\) [0-9.]* [a-zµ]*\].*/\1/p' | head -1)
        
        if [ -n "$TIME_VALUE" ] && [ -n "$TIME_UNIT" ] && [ -n "$CURRENT_BENCH" ] && [ "$TIME_VALUE" != "0" ]; then
            # Convert to nanoseconds
            TIME_NS="0"
            case "$TIME_UNIT" in
                "ns")
                    TIME_NS=$(awk "BEGIN {printf \"%.0f\", $TIME_VALUE}" 2>/dev/null || echo "0")
                    ;;
                "µs"|"us")
                    TIME_NS=$(awk "BEGIN {printf \"%.0f\", $TIME_VALUE * 1000}" 2>/dev/null || echo "0")
                    ;;
                "ms")
                    TIME_NS=$(awk "BEGIN {printf \"%.0f\", $TIME_VALUE * 1000000}" 2>/dev/null || echo "0")
                    ;;
                "s")
                    TIME_NS=$(awk "BEGIN {printf \"%.0f\", $TIME_VALUE * 1000000000}" 2>/dev/null || echo "0")
                    ;;
            esac
            
            if [ "$TIME_NS" != "0" ] && [ -n "$TIME_NS" ]; then
                TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
                # Extract just the function name (e.g., "assemble_block" from "block_assembly/assemble_block")
                CLEAN_NAME=$(echo "$CURRENT_BENCH" | sed 's/.*\///' | sed 's/:$//')
                # Use direct number substitution (no --argjson needed)
                BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$CLEAN_NAME" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $TIME_NS}]" 2>/dev/null || echo "$BENCHMARKS")
            fi
            CURRENT_BENCH=""
        fi
    fi
done <<< "$BENCH_OUTPUT"

# Primary method: Parse from Criterion JSON output (more reliable)
CRITERION_DIR="$BENCH_DIR/target/criterion"
# Look for assemble_block benchmark
bench_dir="$CRITERION_DIR/assemble_block"
if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
    TIME_NS=$(jq -r '.mean.point_estimate' "$bench_dir/base/estimates.json" 2>/dev/null || echo "")
    if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
        TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
        TIME_US=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000}" 2>/dev/null || echo "0")
        # Use direct number substitution (no --argjson needed)
        BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "assemble_block" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_us\": $TIME_US, \"time_ns\": $TIME_NS}]" 2>/dev/null || echo "$BENCHMARKS")
    fi
fi

if [ "$BENCHMARKS" != "[]" ]; then
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmarks (block assembly)",
  "benchmark_suite": "block_assembly",
  "benchmarks": $BENCHMARKS,
  "comparison_note": "Measures create_new_block (assembling blocks from mempool transactions). Fair comparison with Core's AssembleBlock.",
  "log_file": "$LOG_FILE"
}
EOF
    echo "✅ Results saved to: $OUTPUT_FILE"
else
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "No block assembly benchmarks found or execution failed",
  "log_file": "$LOG_FILE"
}
EOF
    exit 1
fi
