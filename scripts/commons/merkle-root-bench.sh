#!/bin/bash
# Bitcoin Commons Merkle Root Benchmark
# Measures merkle root calculation performance using Criterion
set -e
# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"

BENCH_DIR="$BLLVM_BENCH_ROOT"
OUTPUT_FILE="$OUTPUT_DIR/commons-merkle-root-bench-$(date +%Y%m%d-%H%M%S).json"

echo "=== Bitcoin Commons Merkle Root Benchmark ==="
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

echo "Running Commons Merkle Root benchmark..."
echo "Output: $OUTPUT_FILE"

LOG_FILE="/tmp/commons-merkle-root.log"
BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench merkle_tree_precomputed --features production 2>&1 | tee "$LOG_FILE" || echo "")

# Extract from Criterion JSON
CRITERION_DIR="$BENCH_DIR/target/criterion"
TIME_NS="0"
TIME_MS="0"

# Look for merkle root benchmark (may be named differently)
for bench_name in "calculate_merkle_root" "merkle_root" "merkle_tree_precomputed"; do
    BENCH_PATH="$CRITERION_DIR/$bench_name"
    if [ -d "$BENCH_PATH" ] && [ -f "$BENCH_PATH/base/estimates.json" ]; then
        TIME_NS=$(jq -r '.mean.point_estimate // 0' "$BENCH_PATH/base/estimates.json" 2>/dev/null || echo "0")
        if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            break
        fi
    fi
done

# Fallback: parse output for timing
if [ "$TIME_NS" = "0" ] && echo "$BENCH_OUTPUT" | grep -q "merkle"; then
    TIME_LINE=$(echo "$BENCH_OUTPUT" | grep -i "merkle" | head -1 || echo "")
    if [ -n "$TIME_LINE" ]; then
        TIME_VALUE=$(echo "$TIME_LINE" | grep -oE '[0-9]+\.[0-9]+ [mun]?s' | head -1 || echo "")
        if [ -n "$TIME_VALUE" ]; then
            if echo "$TIME_VALUE" | grep -q "ns"; then
                TIME_NS=$(echo "$TIME_VALUE" | grep -oE '[0-9]+\.[0-9]+' | head -1 || echo "0")
            elif echo "$TIME_VALUE" | grep -q "µs\|us"; then
                TIME_US=$(echo "$TIME_VALUE" | grep -oE '[0-9]+\.[0-9]+' | head -1 || echo "0")
                TIME_NS=$(awk "BEGIN {printf \"%.0f\", $TIME_US * 1000}" 2>/dev/null || echo "0")
            elif echo "$TIME_VALUE" | grep -q "ms"; then
                TIME_MS_VAL=$(echo "$TIME_VALUE" | grep -oE '[0-9]+\.[0-9]+' | head -1 || echo "0")
                TIME_NS=$(awk "BEGIN {printf \"%.0f\", $TIME_MS_VAL * 1000000}" 2>/dev/null || echo "0")
            fi
            TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
        fi
    fi
fi

# Generate JSON output
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmark - benches/consensus/merkle_tree_precomputed.rs",
  "benchmark_name": "merkle_root",
  "bitcoin_commons": {
    "time_ms": $TIME_MS,
    "time_ns": $TIME_NS,
    "note": "Measures merkle root calculation from pre-computed transaction hashes (matches Core's MerkleRoot benchmark approach)"
  },
  "comparison_note": "This benchmark measures Commons' merkle root calculation performance. Core has equivalent MerkleRoot benchmark."
}
EOF

echo "✅ Results saved to: $OUTPUT_FILE"
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

