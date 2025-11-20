#!/bin/bash
# Bitcoin Core Compact Block Encoding Benchmark
# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="$RESULTS_DIR_FALLBACK"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/compact-block-encoding-bench-$(date +%Y%m%d-%H%M%S).json"

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
    echo "❌ get_bench_bitcoin function not found after sourcing common.sh"
    exit 0
fi

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/core-compact-block-encoding-bench-$(date +%Y%m%d-%H%M%S).json"

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

echo "=== Bitcoin Core Compact Block Encoding Benchmark ==="
echo ""
echo "Running Core Compact Block Encoding benchmarks..."
echo "Output: $OUTPUT_FILE"

BENCH_OUTPUT=$("$BENCH_BITCOIN" -filter="BlockEncoding" 2>&1 || true)

# Parse bench_bitcoin output - look for BlockEncoding benchmarks
BENCHMARKS="[]"
while IFS= read -r line; do
    if echo "$line" | grep -qE "BlockEncoding"; then
        # Extract benchmark name (in backticks)
        BENCH_NAME=$(echo "$line" | grep -oE '`[^`]+`' | tr -d '`' || echo "")
        if [ -z "$BENCH_NAME" ]; then
            # Try alternative pattern
            BENCH_NAME=$(echo "$line" | awk -F'|' '{print $NF}' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' | grep -oE '[A-Za-z0-9_]+' | head -1 || echo "")
        fi
        
        # Extract time value from first column (ns/op column)
        TIME_VALUE=$(echo "$line" | awk -F'|' '{gsub(/[^0-9.]/,"",$2); print $2}' | head -1 || echo "0")
        
        TIME_NS="0"
        if [ -n "$TIME_VALUE" ] && [ "$TIME_VALUE" != "0" ] && [ "$TIME_VALUE" != "" ]; then
            TIME_NS=$(awk "BEGIN {printf \"%.0f\", $TIME_VALUE}" 2>/dev/null || echo "0")
            TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            
            if [ -n "$BENCH_NAME" ] && [ "$TIME_NS" != "0" ]; then
                BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$BENCH_NAME" ". += [{\"name\": \$name, \"time_ms\": $TIME_MS, \"time_ns\": $TIME_NS}]" 2>/dev/null || echo "$BENCHMARKS")
            fi
        fi
    fi
done <<< "$BENCH_OUTPUT"

# Generate JSON output
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "bench_bitcoin -filter=BlockEncoding",
  "benchmark_name": "compact_block_encoding",
  "bitcoin_core": {
    "benchmarks": $BENCHMARKS,
    "note": "Measures BIP152 compact block encoding performance (BlockEncodingLargeExtra, BlockEncodingNoExtra, BlockEncodingStdExtra)"
  },
  "comparison_note": "This benchmark measures Core's compact block encoding performance. Commons has equivalent benchmarks in compact_blocks.rs."
}
EOF

echo "✅ Results saved to: $OUTPUT_FILE"
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

