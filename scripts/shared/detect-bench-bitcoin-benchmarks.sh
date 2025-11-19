#!/bin/bash
# Auto-detect available benchmarks from bench_bitcoin
# This script extracts all benchmark names from bench_bitcoin output
# and creates a mapping file for use by other scripts

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/common.sh" || {
    echo "❌ Failed to source common.sh" >&2
    exit 1
}

# Find bench_bitcoin
BENCH_BITCOIN=$(get_bench_bitcoin)

if [ -z "$BENCH_BITCOIN" ] || [ ! -f "$BENCH_BITCOIN" ]; then
    echo "❌ bench_bitcoin not found" >&2
    exit 1
fi

echo "🔍 Detecting available benchmarks from: $BENCH_BITCOIN"
echo ""

# Run bench_bitcoin and extract all benchmark names
# Benchmarks appear in backticks: `BenchmarkName`
# Format: | ns/op | op/s | ... | `BenchmarkName`
BENCH_OUTPUT=$("$BENCH_BITCOIN" 2>&1 || true)

# Extract all benchmark names (in backticks)
BENCHMARK_NAMES=$(echo "$BENCH_OUTPUT" | grep -oE '`[^`]+`' | tr -d '`' | sort -u)

if [ -z "$BENCHMARK_NAMES" ]; then
    echo "⚠️  No benchmarks found in output" >&2
    echo "Raw output (first 100 lines):" >&2
    echo "$BENCH_OUTPUT" | head -100 >&2
    exit 1
fi

# Count benchmarks
BENCH_COUNT=$(echo "$BENCHMARK_NAMES" | wc -l)
echo "✅ Found $BENCH_COUNT unique benchmarks"
echo ""

# Create a JSON mapping file
OUTPUT_FILE="${BLLVM_BENCH_ROOT:-$SCRIPT_DIR/../..}/scripts/shared/bench_bitcoin_benchmarks.json"

# Build JSON object with benchmark names and categories
cat > "$OUTPUT_FILE" << EOF
{
  "detected_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "bench_bitcoin_path": "$BENCH_BITCOIN",
  "total_benchmarks": $BENCH_COUNT,
  "benchmarks": [
$(echo "$BENCHMARK_NAMES" | sed 's/^/    "/;s/$/",/' | sed '$s/,$//')
  ],
  "categories": {
EOF

# Categorize benchmarks
MEMPOOL_BENCHMARKS=$(echo "$BENCHMARK_NAMES" | grep -iE "mempool|complex" || true)
TRANSACTION_BENCHMARKS=$(echo "$BENCHMARK_NAMES" | grep -iE "transaction|tx" || true)
BLOCK_BENCHMARKS=$(echo "$BENCHMARK_NAMES" | grep -iE "block|connect" || true)
SCRIPT_BENCHMARKS=$(echo "$BENCHMARK_NAMES" | grep -iE "script|verify" || true)
HASH_BENCHMARKS=$(echo "$BENCHMARK_NAMES" | grep -iE "hash|sha|ripemd|merkle" || true)
ENCODING_BENCHMARKS=$(echo "$BENCHMARK_NAMES" | grep -iE "base58|bech32|encode|decode" || true)
UTXO_BENCHMARKS=$(echo "$BENCHMARK_NAMES" | grep -iE "utxo|coin" || true)

# Add categories to JSON
cat >> "$OUTPUT_FILE" << EOF
    "mempool": [
$(echo "$MEMPOOL_BENCHMARKS" | sed 's/^/      "/;s/$/",/' | sed '$s/,$//' || echo "      null")
    ],
    "transaction": [
$(echo "$TRANSACTION_BENCHMARKS" | sed 's/^/      "/;s/$/",/' | sed '$s/,$//' || echo "      null")
    ],
    "block": [
$(echo "$BLOCK_BENCHMARKS" | sed 's/^/      "/;s/$/",/' | sed '$s/,$//' || echo "      null")
    ],
    "script": [
$(echo "$SCRIPT_BENCHMARKS" | sed 's/^/      "/;s/$/",/' | sed '$s/,$//' || echo "      null")
    ],
    "hash": [
$(echo "$HASH_BENCHMARKS" | sed 's/^/      "/;s/$/",/' | sed '$s/,$//' || echo "      null")
    ],
    "encoding": [
$(echo "$ENCODING_BENCHMARKS" | sed 's/^/      "/;s/$/",/' | sed '$s/,$//' || echo "      null")
    ],
    "utxo": [
$(echo "$UTXO_BENCHMARKS" | sed 's/^/      "/;s/$/",/' | sed '$s/,$//' || echo "      null")
    ]
  }
}
EOF

# Validate JSON
if jq . "$OUTPUT_FILE" >/dev/null 2>&1; then
    echo "✅ Benchmark mapping saved to: $OUTPUT_FILE"
    echo ""
    echo "📊 Summary by category:"
    echo "  Mempool: $(echo "$MEMPOOL_BENCHMARKS" | wc -l) benchmarks"
    echo "  Transaction: $(echo "$TRANSACTION_BENCHMARKS" | wc -l) benchmarks"
    echo "  Block: $(echo "$BLOCK_BENCHMARKS" | wc -l) benchmarks"
    echo "  Script: $(echo "$SCRIPT_BENCHMARKS" | wc -l) benchmarks"
    echo "  Hash: $(echo "$HASH_BENCHMARKS" | wc -l) benchmarks"
    echo "  Encoding: $(echo "$ENCODING_BENCHMARKS" | wc -l) benchmarks"
    echo "  UTXO: $(echo "$UTXO_BENCHMARKS" | wc -l) benchmarks"
    echo ""
    echo "🔍 Key benchmarks found:"
    echo "$BENCHMARK_NAMES" | head -20
    if [ "$BENCH_COUNT" -gt 20 ]; then
        echo "  ... and $((BENCH_COUNT - 20)) more"
    fi
else
    echo "❌ Failed to create valid JSON" >&2
    exit 1
fi

