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

# Try all possible benchmark names (matching Cargo.toml)
for bench_name in "check_block" "transaction_validation"; do
    echo "Trying benchmark: $bench_name"
    # Try without --features production first
    if cargo bench --bench "$bench_name" 2>&1 | tee "$LOG_FILE"; then
        # Verify compilation actually succeeded (check for "error:" or "warning: build failed")
        if grep -q "error:" "$LOG_FILE" || grep -q "warning: build failed" "$LOG_FILE"; then
            echo "⚠️  $bench_name compilation failed, trying with --features production..."
            # Try with production features as fallback
            if cargo bench --bench "$bench_name" --features production 2>&1 | tee -a "$LOG_FILE"; then
                # Check again for compilation errors
                if ! grep -q "error:" "$LOG_FILE" && ! grep -q "warning: build failed" "$LOG_FILE"; then
                    BENCH_SUCCESS=true
                    echo "✅ $bench_name benchmark completed (with production features)"
                    break
                fi
            fi
        else
            BENCH_SUCCESS=true
            echo "✅ $bench_name benchmark completed"
            break
        fi
    else
        echo "⚠️  $bench_name failed, trying with --features production..."
        # Try with production features as fallback
        if cargo bench --bench "$bench_name" --features production 2>&1 | tee -a "$LOG_FILE"; then
            # Check for compilation errors
            if ! grep -q "error:" "$LOG_FILE" && ! grep -q "warning: build failed" "$LOG_FILE"; then
                BENCH_SUCCESS=true
                echo "✅ $bench_name benchmark completed (with production features)"
                break
            fi
        fi
    fi
done

if [ "$BENCH_SUCCESS" = "false" ]; then
    echo "❌ All transaction validation benchmarks failed - check $LOG_FILE"
    echo "   Checking available benchmarks..."
    cargo bench --help 2>&1 | head -5 || true
fi

CRITERION_DIR="$BENCH_DIR/target/criterion"
BENCHMARKS="[]"

# Verify Criterion output exists and has actual data
if [ "$BENCH_SUCCESS" = "true" ]; then
    if [ ! -d "$CRITERION_DIR" ]; then
        echo "⚠️  WARNING: Criterion directory does not exist: $CRITERION_DIR"
        BENCH_SUCCESS=false
    elif ! find "$CRITERION_DIR" -name "estimates.json" -type f | grep -q .; then
        echo "⚠️  WARNING: Criterion directory exists but no estimates.json files found"
        BENCH_SUCCESS=false
    fi
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
            
            # Use direct number substitution (no --argjson needed)
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$BENCH_NAME" \
                --slurpfile stats "$STATS" \
                ". += [{
                    \"name\": \$name,
                    \"time_ms\": $TIME_MS,
                    \"time_ns\": $TIME_NS_INT,
                    "statistics": $stats[0]
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

