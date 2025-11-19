#!/bin/bash
# Bitcoin Core Transaction ID Calculation Benchmark (Portable)

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="$RESULTS_DIR_FALLBACK"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/transaction-id-bench-$(date +%Y%m%d-%H%M%S).json"

# Set trap to ensure JSON is always written, even on unexpected exit
trap 'if [ -n "$OUTPUT_FILE" ] && [ ! -f "$OUTPUT_FILE" ]; then echo "{\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"error\":\"Script exited unexpectedly before writing JSON\",\"script\":\"$0\"}" > "$OUTPUT_FILE" 2>/dev/null || true; fi' EXIT ERR

source "$SCRIPT_DIR/../shared/common.sh" || {
    echo "❌ Failed to source common.sh"
    exit 1
}

# Verify get_bench_bitcoin function is available
if ! type get_bench_bitcoin >/dev/null 2>&1; then
        cat > "$OUTPUT_FILE" << EOF
    {
      "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
      "error": "get_bench_bitcoin function not found",
      "script": "$0"
    }
    EOF
        exit 0
fi

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/core-transaction-id-bench-$(date +%Y%m%d-%H%M%S).json"

# Bitcoin Core Transaction ID Calculation Benchmark
# Measures transaction ID (hash) calculation performance using bench_bitcoin

# Reliably find or build bench_bitcoin
BENCH_BITCOIN=$(get_bench_bitcoin)

if [ -z "$BENCH_BITCOIN" ] || [ ! -f "$BENCH_BITCOIN" ]; then
    echo "❌ bench_bitcoin not found or not executable"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bench_bitcoin not found",
  "core_path": "${CORE_PATH:-not_set}",
  "note": "Please build Core with: cd \$CORE_PATH && cmake -B build -DBUILD_BENCH=ON && cmake --build build -t bench_bitcoin"
}
EOF
    exit 0
fi

echo "Using bench_bitcoin: $BENCH_BITCOIN"
echo "Running Core Transaction ID Calculation benchmark..."
echo "Output: $OUTPUT_FILE"

# Run bench_bitcoin for TransactionIdCalculation benchmark
BENCH_OUTPUT=$("$BENCH_BITCOIN" -filter="TransactionIdCalculation" 2>&1 || echo "")

# Check if bench_bitcoin actually produced output
if [ -z "$BENCH_OUTPUT" ] || ! echo "$BENCH_OUTPUT" | grep -q "TransactionIdCalculation"; then
    echo "⚠️  bench_bitcoin produced no output or no matching benchmarks"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bench_bitcoin produced no output",
  "bench_bitcoin_path": "$BENCH_BITCOIN",
  "output_preview": "${BENCH_OUTPUT:0:500}",
  "benchmarks": []
}
EOF
    exit 0
fi

# Parse bench_bitcoin output
# Format: "TransactionIdCalculation        , 1234.56, 1234.56, 1234.56, 1234.56, 1234.56"
if echo "$BENCH_OUTPUT" | grep -q "TransactionIdCalculation"; then
    TIME_MS=$(echo "$BENCH_OUTPUT" | grep "TransactionIdCalculation" | awk -F',' '{print $2}' | tr -d ' ' || echo "")
    
    if [ -n "$TIME_MS" ] && [ "$TIME_MS" != "null" ] && [ "$TIME_MS" != "0" ]; then
        cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "transaction_id_calculation",
  "implementation": "bitcoin_core",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "methodology": "Transaction ID calculation using GetHash() (double SHA256 of serialized transaction without witness)",
  "benchmarks": [
    {
      "name": "TransactionIdCalculation",
      "time_ms": $TIME_MS,
      "comparison_note": "Measures double SHA256 of serialized transaction (same as Commons' calculate_tx_id)"
    }
  ]
}
EOF
        echo "✓ Transaction ID calculation benchmark completed"
        echo "  Time: ${TIME_MS} ms"
    else
        echo "WARNING: Could not parse time from bench_bitcoin output"
        cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "transaction_id_calculation",
  "implementation": "bitcoin_core",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Could not parse timing from bench_bitcoin output",
  "raw_output": "$(echo "$BENCH_OUTPUT" | head -50 | jq -Rs .)"
}
EOF
    fi
else
    echo "WARNING: Benchmark 'TransactionIdCalculation' not found"
    cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "transaction_id_calculation",
  "implementation": "bitcoin_core",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Benchmark 'TransactionIdCalculation' not found. Please rebuild Core with the new benchmark.",
  "raw_output": "$(echo "$BENCH_OUTPUT" | head -50 | jq -Rs .)"
}
EOF
fi

echo ""
echo "Benchmark data written to: $OUTPUT_FILE"

