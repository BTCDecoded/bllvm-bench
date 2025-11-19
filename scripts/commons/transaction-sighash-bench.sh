#!/bin/bash
# Bitcoin Commons Transaction Sighash Calculation Benchmark
# Measures transaction sighash calculation performance using Criterion
set -e

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
# OUTPUT_DIR already set by get_output_dir # Ensure absolute path
mkdir -p "$OUTPUT_DIR"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"
BENCH_DIR="$BLLVM_BENCH_ROOT"
OUTPUT_FILE="$OUTPUT_DIR/commons-transaction-sighash-bench-$(date +%Y%m%d-%H%M%S).json"

echo "=== Bitcoin Commons Transaction Sighash Calculation Benchmark ==="
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

echo "Running Commons Transaction Sighash Calculation benchmark..."
echo "Output: $OUTPUT_FILE"

LOG_FILE="/tmp/commons-transaction-sighash.log"
# Run hash_operations benchmark which includes sighash benchmarks
BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench hash_operations --features production -- --test-threads=1 2>&1 | tee "$LOG_FILE" || echo "")

BENCHMARKS="[]"

# Primary method: Extract from Criterion JSON (more reliable)
CRITERION_DIR="$BENCH_DIR/target/criterion"
if [ -d "$CRITERION_DIR" ]; then
    # Look for calculate_transaction_sighash benchmark
    if [ -d "$CRITERION_DIR/calculate_transaction_sighash" ] && [ -f "$CRITERION_DIR/calculate_transaction_sighash/base/estimates.json" ]; then
        TIME_NS=$(jq -r '.mean.point_estimate // 0' "$CRITERION_DIR/calculate_transaction_sighash/base/estimates.json" 2>/dev/null || echo "0")
        if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "calculate_transaction_sighash" --arg name "$name" ". += [{"name": $name, "time_ms": $TIME_MS, "time_ns": $TIME_NS}]" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns}]' 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
fi

# Fallback: Parse sighash benchmarks from output
CURRENT_BENCH=""
if [ "$BENCHMARKS" = "[]" ]; then
while IFS= read -r line; do
    # Extract benchmark name from "Benchmarking <name>:" line
    if echo "$line" | grep -qE "^Benchmarking.*sighash"; then
        CURRENT_BENCH=$(echo "$line" | sed 's/^Benchmarking //' | sed 's/:$//' | awk '{print $1}' | tr -d ' ')
    fi
    # Extract time from "time: [min median max]" line
    if echo "$line" | grep -qE "time:\s*\[" && echo "$CURRENT_BENCH" | grep -q "sighash"; then
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
                # Extract just the function name (e.g., "sighash_with_template_check" from "sighash_with_template_check")
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
  "measurement_method": "Criterion benchmarks (transaction sighash calculation)",
  "benchmark_suite": "transaction_sighash",
  "benchmarks": $BENCHMARKS,
  "comparison_note": "Measures calculate_transaction_sighash (hash that gets signed). Fair comparison with Core's SignatureHash.",
  "log_file": "$LOG_FILE"
}
EOF
    echo "✅ Results saved to: $OUTPUT_FILE"
else
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "No transaction sighash benchmarks found or execution failed",
  "log_file": "$LOG_FILE"
}
EOF
    exit 1
fi
