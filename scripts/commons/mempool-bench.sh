#!/bin/bash
# Bitcoin Commons Mempool Operations Benchmark
# Uses consensus-proof/benches/mempool_operations.rs to benchmark mempool operations

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
BENCH_DIR="$BLLVM_BENCH_ROOT"
OUTPUT_FILE="$OUTPUT_DIR/commons-mempool-bench-$(date +%Y%m%d-%H%M%S).json"

echo "=== Bitcoin Commons Mempool Operations Benchmark ==="
echo ""

if [ ! -d "$BENCH_DIR" ]; then
    echo "❌ consensus-proof directory not found at $BENCH_DIR"
    exit 1
fi

cd "$BENCH_DIR"

echo "Running mempool operations benchmarks (this may take 1-2 minutes)..."
BENCH_START=$(date +%s)

BENCH_OUTPUT=$(cargo bench --bench mempool_operations 2>&1 || echo "")

BENCH_END=$(date +%s)
BENCH_TIME=$((BENCH_END - BENCH_START))

# Helper function to parse Criterion time output
parse_criterion_time() {
    local line="$1"
    if [ -z "$line" ]; then
        echo "0"
        return
    fi
    bracket_content=$(echo "$line" | awk -F'[][]' '{print $2}' 2>/dev/null || echo "")
    if [ -n "$bracket_content" ]; then
        median=$(echo "$bracket_content" | awk '{print $3}' 2>/dev/null || echo "0")
        unit=$(echo "$bracket_content" | awk '{print $4}' 2>/dev/null || echo "ns")
    else
        median="0"
        unit="ns"
    fi
    
    if [ -z "$median" ] || [ "$median" = "0" ] || [ "$median" = "" ]; then
        echo "0"
        return
    fi
    
    # Convert to milliseconds
    if [ "$unit" = "ns" ]; then
        echo "$median" | awk '{printf "%.6f", $1 / 1000000}' 2>/dev/null || echo "0"
    elif [ "$unit" = "us" ] || [ "$unit" = "µs" ]; then
        echo "$median" | awk '{printf "%.6f", $1 / 1000}' 2>/dev/null || echo "0"
    elif [ "$unit" = "ms" ]; then
        echo "$median"
    else
        echo "$median" | awk '{printf "%.6f", $1 / 1000000}' 2>/dev/null || echo "0"
    fi
}

# Extract benchmark results
ACCEPT_SIMPLE_LINE=$(echo "$BENCH_OUTPUT" | grep -i "accept_to_memory_pool_simple" | grep "time:" | head -1 || echo "")
ACCEPT_COMPLEX_LINE=$(echo "$BENCH_OUTPUT" | grep -i "accept_to_memory_pool_complex" | grep "time:" | head -1 || echo "")
IS_STANDARD_LINE=$(echo "$BENCH_OUTPUT" | grep -i "is_standard_tx" | grep "time:" | head -1 || echo "")
REPLACEMENT_LINE=$(echo "$BENCH_OUTPUT" | grep -i "replacement_checks" | grep "time:" | head -1 || echo "")

ACCEPT_SIMPLE_MS=$(parse_criterion_time "$ACCEPT_SIMPLE_LINE")
ACCEPT_COMPLEX_MS=$(parse_criterion_time "$ACCEPT_COMPLEX_LINE")
IS_STANDARD_MS=$(parse_criterion_time "$IS_STANDARD_LINE")
REPLACEMENT_MS=$(parse_criterion_time "$REPLACEMENT_LINE")

# Calculate operations per second
ACCEPT_SIMPLE_OPS="0"
ACCEPT_COMPLEX_OPS="0"
IS_STANDARD_OPS="0"
REPLACEMENT_OPS="0"

if [ "$ACCEPT_SIMPLE_MS" != "0" ] && [ -n "$ACCEPT_SIMPLE_MS" ]; then
    ACCEPT_SIMPLE_OPS=$(awk "BEGIN {if ($ACCEPT_SIMPLE_MS > 0) {result = 1000 / $ACCEPT_SIMPLE_MS; printf \"%.0f\", result} else printf \"0\"}" 2>/dev/null || echo "0")
fi
if [ "$ACCEPT_COMPLEX_MS" != "0" ] && [ -n "$ACCEPT_COMPLEX_MS" ]; then
    ACCEPT_COMPLEX_OPS=$(awk "BEGIN {if ($ACCEPT_COMPLEX_MS > 0) {result = 1000 / $ACCEPT_COMPLEX_MS; printf \"%.0f\", result} else printf \"0\"}" 2>/dev/null || echo "0")
fi
if [ "$IS_STANDARD_MS" != "0" ] && [ -n "$IS_STANDARD_MS" ]; then
    IS_STANDARD_OPS=$(awk "BEGIN {if ($IS_STANDARD_MS > 0) {result = 1000 / $IS_STANDARD_MS; printf \"%.0f\", result} else printf \"0\"}" 2>/dev/null || echo "0")
fi
if [ "$REPLACEMENT_MS" != "0" ] && [ -n "$REPLACEMENT_MS" ]; then
    REPLACEMENT_OPS=$(awk "BEGIN {if ($REPLACEMENT_MS > 0) {result = 1000 / $REPLACEMENT_MS; printf \"%.0f\", result} else printf \"0\"}" 2>/dev/null || echo "0")
fi

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "bitcoin_commons_mempool_operations": {
    "accept_to_memory_pool_simple": {
      "time_ms": ${ACCEPT_SIMPLE_MS},
      "operations_per_second": ${ACCEPT_SIMPLE_OPS},
      "note": "Simple transaction acceptance into mempool"
    },
    "accept_to_memory_pool_complex": {
      "time_ms": ${ACCEPT_COMPLEX_MS},
      "operations_per_second": ${ACCEPT_COMPLEX_OPS},
      "note": "Complex transaction (5 inputs, 3 outputs) acceptance"
    },
    "is_standard_tx": {
      "time_ms": ${IS_STANDARD_MS},
      "operations_per_second": ${IS_STANDARD_OPS},
      "note": "Standard transaction check"
    },
    "replacement_checks": {
      "time_ms": ${REPLACEMENT_MS},
      "operations_per_second": ${REPLACEMENT_OPS},
      "note": "Replace-by-fee (RBF) replacement checks"
    },
    "measurement_method": "Criterion benchmark - consensus-proof/benches/mempool_operations.rs"
  }
}
EOF

echo ""
echo "Results saved to: $OUTPUT_FILE"
echo ""
echo "Benchmark summary:"
if [ "$ACCEPT_SIMPLE_MS" != "0" ]; then
    echo "  Accept to mempool (simple): ${ACCEPT_SIMPLE_MS} ms (${ACCEPT_SIMPLE_OPS} ops/sec)"
fi
if [ "$ACCEPT_COMPLEX_MS" != "0" ]; then
    echo "  Accept to mempool (complex): ${ACCEPT_COMPLEX_MS} ms (${ACCEPT_COMPLEX_OPS} ops/sec)"
fi
if [ "$IS_STANDARD_MS" != "0" ]; then
    echo "  Is standard tx: ${IS_STANDARD_MS} ms (${IS_STANDARD_OPS} ops/sec)"
fi
if [ "$REPLACEMENT_MS" != "0" ]; then
    echo "  Replacement checks: ${REPLACEMENT_MS} ms (${REPLACEMENT_OPS} ops/sec)"
fi
echo ""
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"
