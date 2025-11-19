#!/bin/bash
# Bitcoin Commons Low-Level Hash Micro-Benchmarks
# Measures isolated hash functions (SHA256, RIPEMD160, etc.)

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/commons-hash-micro-bench-$(date +%Y%m%d-%H%M%S).json"

echo "=== Bitcoin Commons Low-Level Hash Micro-Benchmarks ==="
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

echo "Running low-level hash micro-benchmarks..."
echo "This measures isolated hash functions (SHA256, double SHA256, RIPEMD160)"
echo ""

LOG_FILE="/tmp/commons-hash-micro.log"
cargo bench --bench hash_operations --features production 2>&1 | tee "$LOG_FILE" || true

CRITERION_DIR="$BENCH_DIR/target/criterion"
BENCHMARKS="[]"

# Extract low-level hash benchmarks
for bench_name in "sha256_1kb" "double_sha256_1kb" "sha_ni_32b" "sha_ni_64b" "sha_ni_1kb" "sha2_crate_32b" "sha2_crate_64b" "sha_ni_double_32b" "sha2_crate_double_32b"; do
    bench_dir="$CRITERION_DIR/$bench_name"
    if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
        TIME_NS=$(jq -r '.mean.point_estimate' "$bench_dir/base/estimates.json" 2>/dev/null || echo "")
        if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            TIME_NS_INT=$(awk "BEGIN {printf \"%.0f\", $TIME_NS}" 2>/dev/null || echo "0")
            
            # Extract statistical data
            STATS=$("$BLLVM_BENCH_ROOT/scripts/shared/extract-criterion-stats.sh" "$bench_dir/base/estimates.json")
            
            # Calculate hashes per second
            HASHES_PER_SEC=$(awk "BEGIN {if ($TIME_NS > 0) {result = 1000000000 / $TIME_NS; printf \"%.2f\", result} else printf \"0\"}" 2>/dev/null || echo "0")
            
            # Use direct number substitution (no --argjson needed)
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$bench_name" \
                --slurpfile stats "$STATS" \
                ". += [{
                    \"name\": \$name,
                    \"time_ms\": $TIME_MS,
                    \"time_ns\": $TIME_NS_INT,
                    \"hashes_per_second\": $HASHES_PER_SEC,
                    \"statistics\": \$stats[0]
                }]" 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
done

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion low-level hash micro-benchmarks",
  "benchmark_suite": "hash_operations",
  "comparison_note": "Isolated hash function performance - measures individual operations without overhead",
  "benchmarks": $BENCHMARKS,
  "log_file": "$LOG_FILE"
}
EOF

echo "✅ Results saved to: $OUTPUT_FILE"
echo ""
cat "$OUTPUT_FILE" | jq '.summary // .benchmarks | length' 2>/dev/null || echo ""

