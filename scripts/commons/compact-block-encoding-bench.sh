#!/bin/bash
# Bitcoin Commons Compact Block Encoding Benchmark
# Measures compact block encoding performance using Criterion
set -e
# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"

BENCH_DIR="$BLLVM_BENCH_ROOT"
OUTPUT_FILE="$OUTPUT_DIR/commons-compact-block-encoding-bench-$(date +%Y%m%d-%H%M%S).json"

echo "=== Bitcoin Commons Compact Block Encoding Benchmark ==="
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

echo "Running Commons Compact Block Encoding benchmark..."
echo "Output: $OUTPUT_FILE"

LOG_FILE="/tmp/commons-compact-blocks.log"
BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench compact_blocks --features production 2>&1 | tee "$LOG_FILE" || echo "")

# Extract from Criterion JSON
CRITERION_DIR="$BENCH_DIR/target/criterion"
BENCHMARKS="[]"

# Look for compact block benchmarks
for bench_name in "create_compact_block" "calculate_tx_hash" "calculate_short_tx_id"; do
    BENCH_PATH="$CRITERION_DIR/$bench_name"
    if [ -d "$BENCH_PATH" ] && [ -f "$BENCH_PATH/base/estimates.json" ]; then
        TIME_NS=$(jq -r '.mean.point_estimate // 0' "$BENCH_PATH/base/estimates.json" 2>/dev/null || echo "0")
        if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$bench_name" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $TIME_NS}]" 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
done

# Generate JSON output
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmark - benches/node/compact_blocks.rs",
  "benchmark_name": "compact_block_encoding",
  "bitcoin_commons": {
    "benchmarks": $BENCHMARKS,
    "note": "Measures BIP152 compact block encoding performance (create_compact_block, calculate_tx_hash, calculate_short_tx_id)"
  },
  "comparison_note": "This benchmark measures Commons' compact block encoding performance. Core has equivalent benchmarks (BlockEncoding*)."
}
EOF

echo "✅ Results saved to: $OUTPUT_FILE"
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

