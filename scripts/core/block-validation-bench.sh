#!/bin/bash
# Bitcoin Core Block Validation Benchmark (Portable)
# Uses bench_bitcoin to benchmark actual ConnectBlock validation

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="$RESULTS_DIR_FALLBACK"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/block-validation-bench-$(date +%Y%m%d-%H%M%S).json"

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

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/core-block-validation-bench-$(date +%Y%m%d-%H%M%S).json"

echo "=== Bitcoin Core Block Validation Benchmark (ConnectBlock) ==="
echo ""

# Reliably find or build bench_bitcoin
BENCH_BITCOIN=$(get_bench_bitcoin)

if [ -z "$BENCH_BITCOIN" ] || [ ! -f "$BENCH_BITCOIN" ]; then
    if [ -n "$CORE_PATH" ]; then
        echo "❌ bench_bitcoin not found"
        echo "   Attempting to build bench_bitcoin..."
        cd "$CORE_PATH"
        if [ -f "Makefile" ]; then
            make -j$(nproc) bench_bitcoin 2>&1 | tail -5 || true
        elif [ -d "build" ]; then
            cd build
            cmake --build . --target bench_bitcoin -j$(nproc) 2>&1 | tail -5 || true
        fi
        
        # Check again
        if [ -f "$CORE_PATH/build/bin/bench_bitcoin" ]; then
            BENCH_BITCOIN="$CORE_PATH/build/bin/bench_bitcoin"
        elif [ -f "$CORE_PATH/bin/bench_bitcoin" ]; then
            BENCH_BITCOIN="$CORE_PATH/bin/bench_bitcoin"
        fi
    fi
    
    if [ -z "$BENCH_BITCOIN" ] || [ ! -f "$BENCH_BITCOIN" ]; then
        echo "❌ Failed to find or build bench_bitcoin"
        echo "   Please build Core with: cd \$CORE_PATH && make bench_bitcoin"
        cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bench_bitcoin not found",
  "core_path": "${CORE_PATH:-not_set}",
  "note": "Please build Core with: make bench_bitcoin"
}
EOF
        echo "✅ Error JSON written to: $OUTPUT_FILE"
        exit 0
    fi
fi

echo "Using bench_bitcoin: $BENCH_BITCOIN"
echo "Running bench_bitcoin for ConnectBlock (this may take 1-2 minutes)..."
echo "This benchmarks actual block validation and connection (ConnectBlock)"

# Run bench_bitcoin and capture output
BENCH_OUTPUT=$("$BENCH_BITCOIN" 2>&1 || echo "")

# Extract ConnectBlock benchmark results
CONNECT_BLOCK_ALL_SCHNORR=$(echo "$BENCH_OUTPUT" | grep -E "ConnectBlockAllSchnorr" | head -1 || echo "")
CONNECT_BLOCK_MIXED=$(echo "$BENCH_OUTPUT" | grep -E "ConnectBlockMixedEcdsaSchnorr" | head -1 || echo "")
CONNECT_BLOCK_ALL_ECDSA=$(echo "$BENCH_OUTPUT" | grep -E "ConnectBlockAllEcdsa" | head -1 || echo "")

# Parse bench_bitcoin output
parse_bench_bitcoin() {
    local line="$1"
    if [ -z "$line" ]; then
        echo "0|0"
        return
    fi
    time_ns=$(echo "$line" | awk -F'|' '{gsub(/[^0-9.]/,"",$2); print $2}' 2>/dev/null || echo "0")
    ops_per_sec=$(echo "$line" | awk -F'|' '{gsub(/[^0-9.]/,"",$3); print $3}' 2>/dev/null || echo "0")
    echo "${time_ns}|${ops_per_sec}"
}

ALL_SCHNORR_DATA=$(parse_bench_bitcoin "$CONNECT_BLOCK_ALL_SCHNORR")
ALL_SCHNORR_TIME_NS=$(echo "$ALL_SCHNORR_DATA" | cut -d'|' -f1)
ALL_SCHNORR_OPS=$(echo "$ALL_SCHNORR_DATA" | cut -d'|' -f2)

MIXED_DATA=$(parse_bench_bitcoin "$CONNECT_BLOCK_MIXED")
MIXED_TIME_NS=$(echo "$MIXED_DATA" | cut -d'|' -f1)
MIXED_OPS=$(echo "$MIXED_DATA" | cut -d'|' -f2)

ALL_ECDSA_DATA=$(parse_bench_bitcoin "$CONNECT_BLOCK_ALL_ECDSA")
ALL_ECDSA_TIME_NS=$(echo "$ALL_ECDSA_DATA" | cut -d'|' -f1)
ALL_ECDSA_OPS=$(echo "$ALL_ECDSA_DATA" | cut -d'|' -f2)

# Convert to milliseconds
ALL_SCHNORR_TIME_MS=$(awk "BEGIN {printf \"%.6f\", $ALL_SCHNORR_TIME_NS / 1000000}" 2>/dev/null || echo "0")
MIXED_TIME_MS=$(awk "BEGIN {printf \"%.6f\", $MIXED_TIME_NS / 1000000}" 2>/dev/null || echo "0")
ALL_ECDSA_TIME_MS=$(awk "BEGIN {printf \"%.6f\", $ALL_ECDSA_TIME_NS / 1000000}" 2>/dev/null || echo "0")

# Use the mixed benchmark as the primary comparison (most realistic)
PRIMARY_TIME_MS="$MIXED_TIME_MS"
PRIMARY_TIME_NS="$MIXED_TIME_NS"
PRIMARY_OPS="$MIXED_OPS"
if [ "$PRIMARY_TIME_MS" = "0" ] || [ -z "$PRIMARY_TIME_MS" ]; then
    PRIMARY_TIME_MS="$ALL_SCHNORR_TIME_MS"
    PRIMARY_TIME_NS="$ALL_SCHNORR_TIME_NS"
    PRIMARY_OPS="$ALL_SCHNORR_OPS"
fi

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "bitcoin_core_block_validation": {
    "connect_block_all_schnorr": {
      "time_per_block_ns": ${ALL_SCHNORR_TIME_NS},
      "time_per_block_ms": ${ALL_SCHNORR_TIME_MS},
      "blocks_per_second": ${ALL_SCHNORR_OPS},
      "implementation": "Chainstate::ConnectBlock (all Schnorr signatures)"
    },
    "connect_block_mixed_ecdsa_schnorr": {
      "time_per_block_ns": ${MIXED_TIME_NS},
      "time_per_block_ms": ${MIXED_TIME_MS},
      "blocks_per_second": ${MIXED_OPS},
      "implementation": "Chainstate::ConnectBlock (mixed ECDSA/Schnorr)",
      "note": "Most realistic - mixed signature types"
    },
    "connect_block_all_ecdsa": {
      "time_per_block_ns": ${ALL_ECDSA_TIME_NS},
      "time_per_block_ms": ${ALL_ECDSA_TIME_MS},
      "blocks_per_second": ${ALL_ECDSA_OPS},
      "implementation": "Chainstate::ConnectBlock (all ECDSA signatures)"
    },
    "primary_comparison": {
      "time_per_block_ms": ${PRIMARY_TIME_MS},
      "time_per_block_ns": ${PRIMARY_TIME_NS},
      "blocks_per_second": ${PRIMARY_OPS},
      "note": "Primary metric for comparison (mixed ECDSA/Schnorr, most realistic)"
    },
    "measurement_method": "bench_bitcoin - Core's actual ConnectBlock implementation"
  }
}
EOF

echo ""
echo "Results saved to: $OUTPUT_FILE"
if [ "$PRIMARY_TIME_MS" != "0" ]; then
    echo "Primary (mixed): ${PRIMARY_TIME_MS} ms per block (${PRIMARY_OPS} blocks/sec)"
fi
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

