#!/bin/bash
# Bitcoin Commons Block Serialization/Deserialization Benchmark
# Measures block serialization/deserialization performance using Criterion
set -e
# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH_DIR="$BLLVM_BENCH_ROOT"

OUTPUT_FILE="$OUTPUT_DIR/commons-block-serialization-bench-$(date +%Y%m%d-%H%M%S).json"

cd "$BENCH_DIR"

echo "Running Commons Block Serialization benchmark..."
echo "Output: $OUTPUT_FILE"

# Run Criterion benchmark for block serialization
BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench performance_focused --features production -- --test-threads=1 --bench "block_serialization" 2>&1 || echo "")

# Parse Criterion output to extract timing information
BENCHMARKS=()

# Extract serialize_header
if echo "$BENCH_OUTPUT" | grep -q "block_serialization/serialize_header"; then
    TIME_LINE=$(echo "$BENCH_OUTPUT" | grep -A 5 "block_serialization/serialize_header" | grep "time:" | head -1 || echo "")
    if [ -n "$TIME_LINE" ]; then
        TIME_NS=$(echo "$TIME_LINE" | sed -n 's/.*\[\([0-9.]*\)[^0-9]*\([0-9.]*\)[^0-9]*\([0-9.]*\)\].*/\2/p' | head -1 || echo "")
        if [ -n "$TIME_NS" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            BENCHMARKS+=("{\"name\":\"block_serialization/serialize_header\",\"time_ns\":$TIME_NS,\"time_ms\":$TIME_MS}")
        fi
    fi
fi

# Extract deserialize_header
if echo "$BENCH_OUTPUT" | grep -q "block_serialization/deserialize_header"; then
    TIME_LINE=$(echo "$BENCH_OUTPUT" | grep -A 5 "block_serialization/deserialize_header" | grep "time:" | head -1 || echo "")
    if [ -n "$TIME_LINE" ]; then
        TIME_NS=$(echo "$TIME_LINE" | sed -n 's/.*\[\([0-9.]*\)[^0-9]*\([0-9.]*\)[^0-9]*\([0-9.]*\)\].*/\2/p' | head -1 || echo "")
        if [ -n "$TIME_NS" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            BENCHMARKS+=("{\"name\":\"block_serialization/deserialize_header\",\"time_ns\":$TIME_NS,\"time_ms\":$TIME_MS}")
        fi
    fi
fi

# Create JSON output
if [ ${#BENCHMARKS[@]} -gt 0 ]; then
    BENCHMARKS_JSON=$(IFS=','; echo "${BENCHMARKS[*]}")
    cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "block_serialization",
  "implementation": "bitcoin_commons",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "methodology": "Block header serialization/deserialization to Bitcoin wire format using serialize_block_header/deserialize_block_header",
  "benchmarks": [$BENCHMARKS_JSON]
}
EOF
    echo "✓ Block serialization benchmark completed"
    echo "  Found ${#BENCHMARKS[@]} benchmark(s)"
else
    echo "WARNING: No block serialization benchmarks found"
    # Escape raw output safely for JSON
    RAW_OUTPUT_JSON=$(echo "$BENCH_OUTPUT" | head -50 | jq -Rs . 2>/dev/null || echo "\"Error encoding output\"")
    cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "block_serialization",
  "implementation": "bitcoin_commons",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "No block serialization benchmarks found. Please rebuild Commons with the new benchmark.",
  "raw_output": $RAW_OUTPUT_JSON
}
EOF
fi

echo ""
echo "Benchmark data written to: $OUTPUT_FILE"
