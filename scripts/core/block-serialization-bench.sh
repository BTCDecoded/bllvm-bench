#!/bin/bash
# Bitcoin Core Block Serialization/Deserialization Benchmark (Portable)

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="$RESULTS_DIR_FALLBACK"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/block-serialization-bench-$(date +%Y%m%d-%H%M%S).json"

# Set trap to ensure JSON is always written, even on unexpected exit
trap 'if [ -n "$OUTPUT_FILE" ] && [ ! -f "$OUTPUT_FILE" ]; then echo "{\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"error\":\"Script exited unexpectedly before writing JSON\",\"script\":\"$0\"}" > "$OUTPUT_FILE" 2>/dev/null || true; fi' EXIT ERR

source "$SCRIPT_DIR/../shared/common.sh" || {
    echo "❌ Failed to source common.sh" >&2
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Failed to source common.sh",
  "script": "$0"
}
EOF
    exit 0
}

# Verify get_bench_bitcoin function is available
if ! type get_bench_bitcoin >/dev/null 2>&1; then
if ! type get_bench_bitcoin >/dev/null 2>&1; then
    echo "❌ get_bench_bitcoin function not found after sourcing common.sh"
    exit 0
fi

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/core-block-serialization-bench-$(date +%Y%m%d-%H%M%S).json"

# Bitcoin Core Block Serialization/Deserialization Benchmark
# Measures block read/write performance using bench_bitcoin

# Reliably find or build bench_bitcoin
BENCH_BITCOIN=$(get_bench_bitcoin)

if [ -z "$BENCH_BITCOIN" ] || [ ! -f "$BENCH_BITCOIN" ]; then
    echo "❌ bench_bitcoin not found or not executable"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bench_bitcoin not found",
  "core_path": "${CORE_PATH:-not_set}",
  "note": "Please build Core with: cd $CORE_PATH && cmake -B build -DBUILD_BENCH=ON && cmake --build build -t bench_bitcoin"
}
EOF
    echo "✅ Error JSON written to: $OUTPUT_FILE"
    exit 0
fi

echo "Running Core Block Serialization benchmark..."
echo "Output: $OUTPUT_FILE"

# Run bench_bitcoin for block read/write operations
BENCH_OUTPUT=$("$BENCH_BITCOIN" -filter="BlockToJsonVerboseWrite|DeserializeAndCheckBlockTest|DeserializeBlockTest" 2>&1 || true)

# Parse bench_bitcoin output
# Format: "ReadBlockBench        , 1234.56, 1234.56, 1234.56, 1234.56, 1234.56"
BENCHMARKS=()

# Extract ReadBlockBench
if echo "$BENCH_OUTPUT" | grep -q "ReadBlockBench"; then
    READ_TIME=$(echo "$BENCH_OUTPUT" | grep "ReadBlockBench" | awk -F',' '{print $2}' | tr -d ' ' || echo "")
    if [ -n "$READ_TIME" ] && [ "$READ_TIME" != "null" ]; then
        BENCHMARKS+=("{\"name\":\"ReadBlockBench\",\"time_ms\":$READ_TIME}")
    fi
fi

# Extract WriteBlockBench
if echo "$BENCH_OUTPUT" | grep -q "WriteBlockBench"; then
    WRITE_TIME=$(echo "$BENCH_OUTPUT" | grep "WriteBlockBench" | awk -F',' '{print $2}' | tr -d ' ' || echo "")
    if [ -n "$WRITE_TIME" ] && [ "$WRITE_TIME" != "null" ]; then
        BENCHMARKS+=("{\"name\":\"WriteBlockBench\",\"time_ms\":$WRITE_TIME}")
    fi
fi

# Extract DeserializeBlockTest
if echo "$BENCH_OUTPUT" | grep -q "DeserializeBlockTest"; then
    DESER_TIME=$(echo "$BENCH_OUTPUT" | grep "DeserializeBlockTest" | awk -F',' '{print $2}' | tr -d ' ' || echo "")
    if [ -n "$DESER_TIME" ] && [ "$DESER_TIME" != "null" ]; then
        BENCHMARKS+=("{\"name\":\"DeserializeBlockTest\",\"time_ms\":$DESER_TIME}")
    fi
fi

# Create JSON output
if [ ${#BENCHMARKS[@]} -gt 0 ]; then
    BENCHMARKS_JSON=$(IFS=','; echo "${BENCHMARKS[*]}")
    cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "block_serialization",
  "implementation": "bitcoin_core",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "methodology": "Block serialization/deserialization using bench_bitcoin (ReadBlock, WriteBlock, DeserializeBlock)",
  "benchmarks": [$BENCHMARKS_JSON]
}
EOF
    echo "✓ Block serialization benchmark completed"
    echo "  Found ${#BENCHMARKS[@]} benchmark(s)"
else
    echo "WARNING: No block serialization benchmarks found"
    cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "block_serialization",
  "implementation": "bitcoin_core",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "No block serialization benchmarks found",
  "raw_output": "$(echo "$BENCH_OUTPUT" | head -50 | jq -Rs .)"
}
EOF
fi

echo ""
echo "Benchmark data written to: $OUTPUT_FILE"
