#!/bin/bash
# Bitcoin Commons Transaction Serialization Benchmark
# Measures transaction serialization performance using Criterion
set -e
# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH_DIR="$BLLVM_BENCH_ROOT"

OUTPUT_FILE="$OUTPUT_DIR/commons-transaction-serialization-bench-$(date +%Y%m%d-%H%M%S).json"

cd "$BENCH_DIR"

echo "Running Commons Transaction Serialization benchmark..."
echo "Output: $OUTPUT_FILE"

# Run Criterion benchmark for transaction serialization
BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench transaction_serialization --features production 2>&1 | tee /tmp/commons-transaction-serialization.log || echo "")

# Primary method: Extract from Criterion JSON (more reliable)
CRITERION_DIR="$BENCH_DIR/target/criterion"
TIME_NS="0"
TIME_MS="0"

if [ -d "$CRITERION_DIR/serialize_transaction" ] && [ -f "$CRITERION_DIR/serialize_transaction/base/estimates.json" ]; then
    TIME_NS=$(jq -r '.mean.point_estimate // 0' "$CRITERION_DIR/serialize_transaction/base/estimates.json" 2>/dev/null || echo "0")
    if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
        TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
    fi
fi

# Fallback: Parse Criterion output to extract timing information
if [ "$TIME_NS" = "0" ] && echo "$BENCH_OUTPUT" | grep -q "transaction/serialize"; then
    # Extract time from Criterion output
    TIME_LINE=$(echo "$BENCH_OUTPUT" | grep -A 5 "transaction/serialize" | grep "time:" | head -1 || echo "")
    
    if [ -n "$TIME_LINE" ]; then
        # Extract median time (middle value in brackets)
        TIME_NS=$(echo "$TIME_LINE" | sed -n 's/.*\[\([0-9.]*\)[^0-9]*\([0-9.]*\)[^0-9]*\([0-9.]*\)\].*/\2/p' | head -1 || echo "")
        
        if [ -n "$TIME_NS" ]; then
            # Convert nanoseconds to milliseconds
            TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            
            cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "transaction_serialization",
  "implementation": "bitcoin_commons",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "methodology": "Transaction serialization to Bitcoin wire format using serialize_transaction",
  "benchmarks": [
    {
      "name": "transaction/serialize",
      "time_ns": $TIME_NS,
      "time_ms": $TIME_MS,
      "comparison_note": "Measures serialization of transaction to Bitcoin wire format (same as Core's serialization)"
    }
  ]
}
EOF
            echo "✓ Transaction serialization benchmark completed"
            echo "  Time: ${TIME_MS} ms (${TIME_NS} ns)"
        else
            echo "WARNING: Could not parse time from Criterion output"
            # Escape raw output safely for JSON
            RAW_OUTPUT_JSON=$(echo "$BENCH_OUTPUT" | head -50 | jq -Rs . 2>/dev/null || echo "\"Error encoding output\"")
            cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "transaction_serialization",
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
  "benchmark": "transaction_serialization",
  "implementation": "bitcoin_commons",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Could not find timing information",
  "raw_output": $RAW_OUTPUT_JSON
}
EOF
    fi
else
    echo "WARNING: Benchmark 'transaction/serialize' not found in output"
    # Escape raw output safely for JSON
    RAW_OUTPUT_JSON=$(echo "$BENCH_OUTPUT" | head -50 | jq -Rs . 2>/dev/null || echo "\"Error encoding output\"")
    cat > "$OUTPUT_FILE" << EOF
{
  "benchmark": "transaction_serialization",
  "implementation": "bitcoin_commons",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Benchmark 'transaction/serialize' not found",
  "raw_output": $RAW_OUTPUT_JSON
}
EOF
fi

echo ""
echo "Benchmark data written to: $OUTPUT_FILE"
