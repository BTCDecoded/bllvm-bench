#!/bin/bash
# Bitcoin Commons Mempool Operations Benchmark
# Measures mempool operations using Criterion

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/commons-mempool-operations-bench-$(date +%Y%m%d-%H%M%S).json"
BENCH_DIR="$BLLVM_BENCH_ROOT"

echo "=== Bitcoin Commons Mempool Operations Benchmark ==="
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

echo "Running mempool operations benchmarks..."
LOG_FILE="/tmp/commons-mempool.log"
BENCH_SUCCESS=false

# Define Criterion directory BEFORE using it
CRITERION_DIR="$BENCH_DIR/target/criterion"

# Try all possible benchmark names (matching Cargo.toml)
for bench_name in "mempool_operations"; do
    echo "Trying benchmark: $bench_name"
    # Try without --features production first
    if cargo bench --bench "$bench_name" 2>&1 | tee "$LOG_FILE"; then
        # Check if compilation actually succeeded
        if grep -q "error:" "$LOG_FILE" || grep -q "warning: build failed" "$LOG_FILE"; then
            echo "⚠️  $bench_name compilation failed, trying with --features production..."
            # Try with production features as fallback
            if cargo bench --bench "$bench_name" --features production 2>&1 | tee -a "$LOG_FILE"; then
                # Check again for compilation errors
                if ! grep -q "error:" "$LOG_FILE" && ! grep -q "warning: build failed" "$LOG_FILE"; then
                    # Verify Criterion output was actually generated
                    if [ -d "$CRITERION_DIR" ] && find "$CRITERION_DIR" -name "estimates.json" -type f | grep -q .; then
                        BENCH_SUCCESS=true
                        echo "✅ $bench_name benchmark completed (with production features)"
                        break
                    fi
                fi
            fi
        else
            # Verify Criterion output was actually generated
            if [ -d "$CRITERION_DIR" ] && find "$CRITERION_DIR" -name "estimates.json" -type f | grep -q .; then
                BENCH_SUCCESS=true
                echo "✅ $bench_name benchmark completed"
                break
            else
                echo "⚠️  $bench_name ran but no Criterion output found, trying with --features production..."
                # Try with production features as fallback
                if cargo bench --bench "$bench_name" --features production 2>&1 | tee -a "$LOG_FILE"; then
                    if [ -d "$CRITERION_DIR" ] && find "$CRITERION_DIR" -name "estimates.json" -type f | grep -q .; then
                        BENCH_SUCCESS=true
                        echo "✅ $bench_name benchmark completed (with production features)"
                        break
                    fi
                fi
            fi
        fi
    else
        echo "⚠️  $bench_name failed, trying with --features production..."
        # Try with production features as fallback
        if cargo bench --bench "$bench_name" --features production 2>&1 | tee -a "$LOG_FILE"; then
            # Check for compilation errors
            if ! grep -q "error:" "$LOG_FILE" && ! grep -q "warning: build failed" "$LOG_FILE"; then
                # Verify Criterion output was actually generated
                if [ -d "$CRITERION_DIR" ] && find "$CRITERION_DIR" -name "estimates.json" -type f | grep -q .; then
                    BENCH_SUCCESS=true
                    echo "✅ $bench_name benchmark completed (with production features)"
                    break
                fi
            fi
        fi
    fi
done

if [ "$BENCH_SUCCESS" = "false" ]; then
    echo "❌ All mempool benchmarks failed - check $LOG_FILE"
    echo "   Checking available benchmarks..."
    cargo bench --help 2>&1 | head -5 || true
fi

# Extract from Criterion JSON files (more reliable than parsing stdout)
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
  "measurement_method": "Criterion benchmarks (actual mempool operations)",
  "benchmark_suite": "mempool_operations",
  "benchmarks": [],
  "note": "Check log file for details"
}
EOF
    exit 0
fi

# Look for mempool operation benchmarks
for bench_dir in "$CRITERION_DIR"/accept_to_memory_pool* "$CRITERION_DIR"/is_standard_tx* "$CRITERION_DIR"/replacement_checks* "$CRITERION_DIR"/mempool_*; do
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
  "measurement_method": "Criterion benchmarks (actual mempool operations)",
  "benchmark_suite": "mempool_operations",
  "benchmarks": $BENCHMARKS,
  "log_file": "$LOG_FILE"
}
EOF
echo "✅ Results saved to: $OUTPUT_FILE"
