#!/bin/bash
# Bitcoin Commons Script Verification Benchmark
# Measures script verification performance using Criterion
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/commons-script-verification-bench-$(date +%Y%m%d-%H%M%S).json"
BENCH_DIR="$BLLVM_BENCH_ROOT"

cd "$BENCH_DIR"

echo "Running Commons Script Verification benchmark..."
echo "Output: $OUTPUT_FILE"

LOG_FILE="/tmp/commons-script-verification.log"
# Run script verification benchmark
BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench script_verification --features production -- --test-threads=1 2>&1 | tee "$LOG_FILE" || echo "")

BENCHMARKS="[]"

# Primary method: Parse from Criterion JSON output (more reliable)
CRITERION_DIR="$BENCH_DIR/target/criterion"
# Look for verify_script and eval_script_complex benchmarks
for bench_name in "verify_script" "eval_script_complex"; do
    bench_dir="$CRITERION_DIR/$bench_name"
    if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
        TIME_NS=$(jq -r '.mean.point_estimate // 0' "$bench_dir/base/estimates.json" 2>/dev/null || echo "0")
        if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$bench_name" --arg name "$name" ". += [{"name": $name, "time_ms": $TIME_MS, "time_ns": $TIME_NS}]" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns}]' 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
done

# Fallback: Parse script_verification benchmarks from output
CURRENT_BENCH=""
if [ "$BENCHMARKS" = "[]" ]; then
while IFS= read -r line; do
    # Extract benchmark name from "Benchmarking <name>:" line
    if echo "$line" | grep -qE "^Benchmarking.*script_verification"; then
        CURRENT_BENCH=$(echo "$line" | sed 's/^Benchmarking //' | sed 's/:$//' | awk '{print $1}' | tr -d ' ')
    fi
    # Extract time from "time: [min median max]" line (supports ns, µs, ms, s)
    if echo "$line" | grep -qE "time:\s*\[" && echo "$CURRENT_BENCH" | grep -q "script_verification"; then
        if [ -z "$CURRENT_BENCH" ]; then
            # Try to extract from the line itself (benchmark name before "time:")
            CURRENT_BENCH=$(echo "$line" | awk '{print $1}' | tr -d ' ' | tr -d ':')
        fi
        # Extract median time and unit from [min unit median unit max unit] format
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
                TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
                # Extract just the function name (e.g., "verify_script" from "script_verification/verify_script")
                CLEAN_NAME=$(echo "$CURRENT_BENCH" | sed 's/.*\///' | sed 's/:$//')
                BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$CLEAN_NAME" --arg name "$name" ". += [{"name": $name, "time_ms": $TIME_MS, "time_ns": $TIME_NS}]" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns}]' 2>/dev/null || echo "$BENCHMARKS")
            fi
            CURRENT_BENCH=""
        fi
    fi
done <<< "$BENCH_OUTPUT"
fi

if [ "$BENCHMARKS" != "[]" ]; then
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmarks (script verification)",
  "benchmark_suite": "script_verification",
  "benchmarks": $BENCHMARKS,
  "log_file": "$LOG_FILE"
}
EOF
    echo "✅ Results saved to: $OUTPUT_FILE"
else
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "No script verification benchmarks found or execution failed",
  "log_file": "$LOG_FILE"
}
EOF
    exit 1
fi
