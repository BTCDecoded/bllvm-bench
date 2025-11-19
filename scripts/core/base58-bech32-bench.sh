#!/bin/bash
# Bitcoin Core Base58/Bech32 Benchmark (Portable)

# Use set -e but trap errors to ensure JSON is always written
set -e
# Trap will be set after OUTPUT_FILE is defined

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="${RESULTS_DIR_FALLBACK}"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/core-base58-bech32-bench-$(date +%Y%m%d-%H%M%S).json"

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
    echo "❌ get_bench_bitcoin function not found after sourcing common.sh" >&2
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
OUTPUT_FILE="$OUTPUT_DIR/core-base58-bech32-bench-$(date +%Y%m%d-%H%M%S).json"

# Bitcoin Core Base58/Bech32 Benchmark
# Measures Base58 and Bech32 encoding/decoding performance using bench_bitcoin

# Reliably find or build bench_bitcoin
BENCH_BITCOIN=$(get_bench_bitcoin)

if [ -z "$BENCH_BITCOIN" ] || [ ! -f "$BENCH_BITCOIN" ]; then
    echo "❌ bench_bitcoin not found or not executable"
    echo "   BENCH_BITCOIN: ${BENCH_BITCOIN:-empty}"
    echo "   CORE_PATH: ${CORE_PATH:-not_set}"
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bench_bitcoin not found",
  "core_path": "${CORE_PATH:-not_set}",
  "note": "Please build Core with: cd \$CORE_PATH && cmake -B build -DBUILD_BENCH=ON && cmake --build build -t bench_bitcoin"
}
EOF
    echo "✅ Error JSON written to: $OUTPUT_FILE"
    exit 0
fi

echo "Using bench_bitcoin: $BENCH_BITCOIN"
echo "Running bench_bitcoin for Base58/Bech32 operations (this may take a few minutes)..."
echo "This benchmarks Base58 and Bech32 encoding/decoding performance."

# Run bench_bitcoin and capture output
BENCH_OUTPUT=$("$BENCH_BITCOIN" -filter="Base58|Bech32" 2>&1 || echo "")

# Check if bench_bitcoin actually produced output
if [ -z "$BENCH_OUTPUT" ] || ! echo "$BENCH_OUTPUT" | grep -qE "(Base58|Bech32)"; then
    echo "⚠️  bench_bitcoin produced no output or no matching benchmarks"
    echo "   Output preview: ${BENCH_OUTPUT:0:200}"
    # Still write JSON but with error
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

BASE58_ENCODE_LINE=$(echo "$BENCH_OUTPUT" | grep -iE "Base58Encode" | head -1 || echo "")
BASE58_CHECK_ENCODE_LINE=$(echo "$BENCH_OUTPUT" | grep -iE "Base58CheckEncode" | head -1 || echo "")
BASE58_DECODE_LINE=$(echo "$BENCH_OUTPUT" | grep -iE "Base58Decode" | head -1 || echo "")
BECH32_ENCODE_LINE=$(echo "$BENCH_OUTPUT" | grep -iE "Bech32Encode" | head -1 || echo "")
BECH32_DECODE_LINE=$(echo "$BENCH_OUTPUT" | grep -iE "Bech32Decode" | head -1 || echo "")

BENCHMARKS="[]"

for bench_name in "Base58Encode" "Base58CheckEncode" "Base58Decode" "Bech32Encode" "Bech32Decode"; do
    case "$bench_name" in
        "Base58Encode") LINE="$BASE58_ENCODE_LINE" ;;
        "Base58CheckEncode") LINE="$BASE58_CHECK_ENCODE_LINE" ;;
        "Base58Decode") LINE="$BASE58_DECODE_LINE" ;;
        "Bech32Encode") LINE="$BECH32_ENCODE_LINE" ;;
        "Bech32Decode") LINE="$BECH32_DECODE_LINE" ;;
    esac
    
    if [ -n "$LINE" ]; then
        DATA=$(parse_bench_bitcoin "$LINE")
        TIME_NS=$(echo "$DATA" | cut -d'|' -f1)
        OPS=$(echo "$DATA" | cut -d'|' -f2)
        
        if [ "$TIME_NS" != "0" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.6f\", $TIME_NS / 1000000}")
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$bench_name" --arg time "$TIME_MS" --arg timens "$TIME_NS" '. += [{"name": $name, "time_ms": ($time | tonumber), "time_ns": ($timens | tonumber), "ops_per_sec": ($OPS | tonumber)}]' 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
done

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "bench_bitcoin (Base58/Bech32 encoding/decoding)",
  "benchmarks": $BENCHMARKS
}
EOF
echo "✅ Results saved to: $OUTPUT_FILE"


