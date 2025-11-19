#!/bin/bash
# Bitcoin Commons Transaction ID Calculation Benchmark
# Measures transaction ID calculation performance using Criterion
set -e
# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH_DIR="$BLLVM_BENCH_ROOT"

OUTPUT_FILE="$OUTPUT_DIR/commons-transaction-id-bench-$(date +%Y%m%d-%H%M%S).json"

cd "$BENCH_DIR"

echo "Running Commons Transaction ID Calculation benchmark..."
echo "Output: $OUTPUT_FILE"

# Run Criterion benchmark for transaction ID calculation
BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench transaction_id --features production 2>&1 | tee /tmp/commons-transaction-id.log || echo "")

# Primary method: Extract from Criterion JSON (more reliable)
CRITERION_DIR="$BENCH_DIR/target/criterion"
TIME_NS="0"
TIME_MS="0"

if [ -d "$CRITERION_DIR/calculate_tx_id" ] && [ -f "$CRITERION_DIR/calculate_tx_id/base/estimates.json" ]; then
    TIME_NS=$(jq -r '.mean.point_estimate // 0' "$CRITERION_DIR/calculate_tx_id/base/estimates.json" 2>/dev/null || echo "0")
    if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
        TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
    fi
fi

# Fallback: Parse Criterion output to extract timing information
if [ "$TIME_NS" = "0" ] && echo "$BENCH_OUTPUT" | grep -q "transaction/calculate_id"; then
    # Extract time from Criterion output
    # Format: "transaction/calculate_id    time:   [123.45 ns 124.56 ns 125.67 ns]"
    TIME_LINE=$(echo "$BENCH_OUTPUT" | grep -A 5 "transaction/calculate_id" | grep "time:" | head -1 || echo "")
    
    if [ -n "$TIME_LINE" ]; then
        # Extract median time (middle value in brackets)
        # Example: [123.45 ns 124.56 ns 125.67 ns] -> 124.56
        TIME_NS=$(echo "$TIME_LINE" | sed -n 's/.*\[\([0-9.]*\)[^0-9]*\([0-9.]*\)[^0-9]*\([0-9.]*\)\].*/\2/p' | head -1 || echo "")
        
        if [ -n "$TIME_NS" ]; then
            # Convert nanoseconds to milliseconds
            TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            
            cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "transaction_id_calculation",
  "implementation": "bitcoin_commons",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "methodology": "Transaction ID calculation using calculate_tx_id (double SHA256 of serialized transaction)",
  "benchmarks": [
    {
      "name": "transaction/calculate_id",
      "time_ns": $TIME_NS,
      "time_ms": $TIME_MS,
      "comparison_note": "Measures double SHA256 of serialized transaction (same as Core's transaction ID calculation)"
    }
  ]
}
EOF
            echo "✓ Transaction ID calculation benchmark completed"
            echo "  Time: ${TIME_MS} ms (${TIME_NS} ns)"
        else
            echo "WARNING: Could not parse time from Criterion output"
            # Escape raw output safely for JSON
            RAW_OUTPUT_JSON=$(echo "$BENCH_OUTPUT" | head -50 | jq -Rs . 2>/dev/null || echo "\"Error encoding output\"")
            cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "transaction_id_calculation",
  "implementation": "bitcoin_commons",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Could not parse timing from Criterion output",
  "raw_output": $RAW_OUTPUT_JSON
}
EOF
        fi
    else
        echo "WARNING: Could not find timing information in Criterion output"
        # Escape raw output safely for JSON
        RAW_OUTPUT_JSON=$(echo "$BENCH_OUTPUT" | head -50 | jq -Rs . 2>/dev/null || echo "\"Error encoding output\"")
        cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "transaction_id_calculation",
  "implementation": "bitcoin_commons",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Could not find timing information",
  "raw_output": $RAW_OUTPUT_JSON
}
EOF
    fi
else
    echo "WARNING: Benchmark 'transaction/calculate_id' not found in output"
    # Escape raw output safely for JSON
    RAW_OUTPUT_JSON=$(echo "$BENCH_OUTPUT" | head -50 | jq -Rs . 2>/dev/null || echo "\"Error encoding output\"")
    cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "transaction_id_calculation",
  "implementation": "bitcoin_commons",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Benchmark 'transaction/calculate_id' not found",
  "raw_output": $RAW_OUTPUT_JSON
}
EOF
fi

echo ""
echo "Benchmark data written to: $OUTPUT_FILE"
