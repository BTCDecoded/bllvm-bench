#!/bin/bash
# Bitcoin Commons Block Validation Benchmark (Portable)
# Uses bllvm-bench to benchmark actual block validation

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/commons-block-validation-bench-$(date +%Y%m%d-%H%M%S).json"

echo "=== Bitcoin Commons Block Validation Benchmark ==="
echo ""

# bllvm-bench is always in the same repo
BENCH_DIR="$BLLVM_BENCH_ROOT"

if [ ! -d "$BENCH_DIR" ]; then
    echo "❌ bllvm-bench directory not found at $BENCH_DIR"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bllvm-bench directory not found",
  "note": "This script should be run from within bllvm-bench"
}
EOF
    exit 1
fi

cd "$BENCH_DIR"

echo "Running block validation benchmarks (this may take 2-3 minutes)..."
BENCH_START=$(date +%s)

# Run block validation benchmarks with production features
echo "Running block validation benchmarks with production features..."
cargo bench --bench block_validation_realistic --features production 2>&1 | tee /tmp/block_validation_bench.log || {
    echo "Realistic benchmark failed, trying basic benchmark..."
    cargo bench --bench block_validation --features production 2>&1 | tee /tmp/block_validation_bench.log || true
}

BENCH_END=$(date +%s)
BENCH_TIME=$((BENCH_END - BENCH_START))

# Extract from Criterion JSON files
CRITERION_DIR="$BENCH_DIR/target/criterion"
CONNECT_BLOCK_TIME_MS="0"
CONNECT_BLOCK_MULTI_TX_TIME_MS="0"

echo "Searching for Criterion results in: $CRITERION_DIR"

# Try to find any connect_block benchmark (search more broadly)
ESTIMATES_FILE=""

# Search for estimates.json files related to connect_block
if [ -d "$CRITERION_DIR" ]; then
    # Try specific paths first
    POSSIBLE_PATHS=(
        "$CRITERION_DIR/connect_block_realistic_1000tx/base/estimates.json"
        "$CRITERION_DIR/connect_block_realistic_100tx/base/estimates.json"
        "$CRITERION_DIR/connect_block/base/estimates.json"
        "$CRITERION_DIR/block_validation_realistic/connect_block/base/estimates.json"
        "$CRITERION_DIR/block_validation/connect_block/base/estimates.json"
    )
    
    for path in "${POSSIBLE_PATHS[@]}"; do
        if [ -f "$path" ]; then
            ESTIMATES_FILE="$path"
            echo "Found estimates.json at: $path"
            break
        fi
    done
    
    # If not found, search more broadly
    if [ -z "$ESTIMATES_FILE" ]; then
        ESTIMATES_FILE=$(find "$CRITERION_DIR" -name "estimates.json" -path "*/connect_block*" -path "*/base/*" 2>/dev/null | head -1)
        if [ -n "$ESTIMATES_FILE" ]; then
            echo "Found estimates.json by search: $ESTIMATES_FILE"
        fi
    fi
    
    # Last resort: find any estimates.json with "connect" in path
    if [ -z "$ESTIMATES_FILE" ]; then
        ESTIMATES_FILE=$(find "$CRITERION_DIR" -name "estimates.json" -path "*connect*" 2>/dev/null | head -1)
        if [ -n "$ESTIMATES_FILE" ]; then
            echo "Found estimates.json (connect-related): $ESTIMATES_FILE"
        fi
    fi
fi

if [ -n "$ESTIMATES_FILE" ] && [ -f "$ESTIMATES_FILE" ]; then
    # Try mean first, then median
    TIME_NS=$(jq -r '.mean.point_estimate // .median.point_estimate // 0' "$ESTIMATES_FILE" 2>/dev/null || echo "0")
    
    if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ] && [ "$TIME_NS" != "" ]; then
        CONNECT_BLOCK_TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
        echo "✅ Extracted timing: ${CONNECT_BLOCK_TIME_MS} ms per block (${TIME_NS} ns)"
    else
        echo "⚠️  No valid timing found in estimates.json (value: $TIME_NS)"
        echo "   File content preview:"
        jq '.mean // .median' "$ESTIMATES_FILE" 2>/dev/null | head -5 || echo "   Failed to parse JSON"
    fi
else
    echo "⚠️  Criterion estimates.json not found for connect_block"
    if [ -d "$CRITERION_DIR" ]; then
        echo "   Available Criterion benchmarks:"
        find "$CRITERION_DIR" -type d -maxdepth 2 2>/dev/null | head -10 || echo "   (none found)"
    else
        echo "   Criterion directory does not exist: $CRITERION_DIR"
        echo "   Benchmark may have failed to run or output was not generated"
    fi
fi

# Convert to nanoseconds and calculate operations per second
CONNECT_BLOCK_TIME_NS="0"
CONNECT_BLOCK_OPS_PER_SEC="0"
if [ "$CONNECT_BLOCK_TIME_MS" != "0" ] && [ -n "$CONNECT_BLOCK_TIME_MS" ]; then
    CONNECT_BLOCK_TIME_NS=$(awk "BEGIN {printf \"%.0f\", $CONNECT_BLOCK_TIME_MS * 1000000}" 2>/dev/null || echo "0")
    CONNECT_BLOCK_OPS_PER_SEC=$(awk "BEGIN {if ($CONNECT_BLOCK_TIME_NS > 0) {result = 1000000000 / $CONNECT_BLOCK_TIME_NS; printf \"%.2f\", result} else printf \"0\"}" 2>/dev/null || echo "0")
fi

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "bitcoin_commons_block_validation": {
    "connect_block": {
      "time_per_block_ms": ${CONNECT_BLOCK_TIME_MS},
      "time_per_block_ns": ${CONNECT_BLOCK_TIME_NS},
      "blocks_per_second": ${CONNECT_BLOCK_OPS_PER_SEC},
      "implementation": "bllvm-consensus::block::connect_block with real P2WPKH signatures",
      "note": "Actual block validation with 1000 transactions and real ECDSA signatures",
      "benchmark_source": "bllvm-bench/benches/consensus/block_validation_realistic.rs"
    },
    "measurement_method": "Criterion benchmark - bllvm-bench/benches/consensus/block_validation_realistic.rs"
  }
}
EOF

echo ""
echo "Results saved to: $OUTPUT_FILE"
if [ "$CONNECT_BLOCK_TIME_MS" != "0" ]; then
    echo "connect_block: ${CONNECT_BLOCK_TIME_MS} ms per block (${CONNECT_BLOCK_OPS_PER_SEC} blocks/sec)"
fi
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

