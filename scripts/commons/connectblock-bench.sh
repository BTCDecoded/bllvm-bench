#!/bin/bash
# Bitcoin Commons ConnectBlock Benchmark
# Measures connect_block with 1000 transactions using Criterion

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/commons-connectblock-bench-$(date +%Y%m%d-%H%M%S).json"
BENCH_DIR="$BLLVM_BENCH_ROOT"

echo "=== Bitcoin Commons ConnectBlock Benchmark (1000 transactions) ==="
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

echo "Running ConnectBlock benchmark with 1000 transactions..."
LOG_FILE="/tmp/commons-connectblock.log"
if cargo bench --bench block_validation_realistic --features production 2>&1 | tee "$LOG_FILE"; then
    # Extract from Criterion JSON (more reliable than stdout parsing)
    CRITERION_DIR="$BENCH_DIR/target/criterion"
    BENCHMARKS="[]"
    
    # Look for connect_block_realistic_1000tx benchmark
    # Report generator looks for "connect_block_1000tx" but we have "connect_block_realistic_1000tx"
    # Output both names for compatibility
    bench_dir="$CRITERION_DIR/connect_block_realistic_1000tx"
    if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
        TIME_NS=$(jq -r '.mean.point_estimate // 0' "$bench_dir/base/estimates.json" 2>/dev/null || echo "0")
        if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "0" ] && [ "$TIME_NS" != "null" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            # Add both names: realistic (actual) and 1000tx (for report generator matching)
            # Use direct number substitution (no --argjson needed)
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "connect_block_realistic_1000tx" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $TIME_NS}]" 2>/dev/null || echo "$BENCHMARKS")
            # Also add alias for report generator
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "connect_block_1000tx" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $TIME_NS}]" 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
    
    # Fallback to stdout parsing if JSON not found
    if [ "$BENCHMARKS" = "[]" ]; then
        CURRENT_BENCH=""
        while IFS= read -r line; do
            if echo "$line" | grep -qE "^Benchmarking"; then
                CURRENT_BENCH=$(echo "$line" | sed 's/^Benchmarking //' | sed 's/:$//' | awk '{print $1}' | tr -d ' ')
            fi
            if echo "$line" | grep -qE "time:\s*\["; then
                if [ -z "$CURRENT_BENCH" ]; then
                    CURRENT_BENCH=$(echo "$line" | awk '{print $1}' | tr -d ' ' | tr -d ':')
                fi
                TIME_VALUE=$(echo "$line" | sed -n 's/.*\[[0-9.]* [a-zµ]* \([0-9.]*\) \([a-zµ]*\) [0-9.]* [a-zµ]*\].*/\1/p' | head -1)
                TIME_UNIT=$(echo "$line" | sed -n 's/.*\[[0-9.]* [a-zµ]* [0-9.]* \([a-zµ]*\) [0-9.]* [a-zµ]*\].*/\1/p' | head -1)
                
                # Extract connect_block_realistic_1000tx benchmark
                if echo "$CURRENT_BENCH" | grep -qE "connect_block_realistic_1000tx"; then
                    if [ -n "$TIME_VALUE" ] && [ -n "$TIME_UNIT" ] && [ -n "$CURRENT_BENCH" ] && [ "$TIME_VALUE" != "0" ]; then
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
                            CLEAN_NAME=$(echo "$CURRENT_BENCH" | sed 's/:$//')
                            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$CLEAN_NAME" --arg name "$name" ". += [{"name": $name, "time_ms": $TIME_MS, "time_ns": $TIME_NS}]" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns}]' 2>/dev/null || echo "$BENCHMARKS")
                        fi
                        CURRENT_BENCH=""
                    fi
                fi
            fi
        done < "$LOG_FILE"
    fi
    
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmarks (connect_block with 1000 transactions)",
  "benchmark_suite": "block_validation",
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
