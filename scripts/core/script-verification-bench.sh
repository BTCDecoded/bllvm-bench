#!/bin/bash
# Bitcoin Core Script Verification Benchmark (Portable)

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="$RESULTS_DIR_FALLBACK"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/script-verification-bench-$(date +%Y%m%d-%H%M%S).json"

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
OUTPUT_FILE="$OUTPUT_DIR/core-script-verification-bench-$(date +%Y%m%d-%H%M%S).json"

# Bitcoin Core Script Verification Benchmark
# Measures script verification performance using bench_bitcoin

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
    echo "✅ Error JSON written to: $OUTPUT_FILE"
    exit 0
fi

echo "Using bench_bitcoin: $BENCH_BITCOIN"
echo "Running Core Script Verification benchmark..."
echo "Output: $OUTPUT_FILE"

BENCH_OUTPUT=$("$BENCH_BITCOIN" -filter="VerifyScriptBench|VerifyNestedIfScript" 2>&1 || true)

# Parse bench_bitcoin output - look for the table format
# Format: | TIME_VALUE | op/s | ... | `BenchmarkName`
BENCHMARKS="[]"
while IFS= read -r line; do
    # Look for lines with VerifyScriptBench or VerifyNestedIfScript in the benchmark column
    if echo "$line" | grep -qE "VerifyScriptBench|VerifyNestedIfScript"; then
        # Extract benchmark name (last column, remove backticks)
        BENCH_NAME=$(echo "$line" | grep -oE "VerifyScriptBench|VerifyNestedIfScript" | head -1 || echo "")
        # Extract time value from first column (ns/op column)
        # Format: | 34,604.58 | ... (first number after |)
        TIME_VALUE=$(echo "$line" | awk -F'|' '{gsub(/[^0-9.]/,"",$2); print $2}' | head -1 || echo "0")
        # bench_bitcoin always outputs in ns/op format
        TIME_UNIT="ns"

        TIME_NS="0"
        case "$TIME_UNIT" in
            "ns")
                TIME_NS=$(awk "BEGIN {printf \"%.0f\", $TIME_VALUE}" 2>/dev/null || echo "0")
                ;;
            "µs"|"us")
                TIME_NS=$(awk "BEGIN {printf \"%.0f\", $TIME_VALUE * 1000}" 2>/dev/null || echo "0")
                ;;
            "ms")
                TIME_NS=$(awk "BEGIN {printf \"%.0f\", $TIME_VALUE * 1000000}" 2>/dev/null || echo "0")
                ;;
            "s")
                TIME_NS=$(awk "BEGIN {printf \"%.0f\", $TIME_VALUE * 1000000000}" 2>/dev/null || echo "0")
                ;;
        esac

        if [ -n "$BENCH_NAME" ] && [ "$TIME_NS" != "0" ] && [ -n "$TIME_NS" ]; then
            TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$BENCH_NAME" --arg name "$name" ". += [{"name": $name, "time_ms": $TIME_MS, "time_ns": $TIME_NS}]" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns}]' 2>/dev/null || echo "$BENCHMARKS")
        fi
    fi
done <<< "$BENCH_OUTPUT"

if [ "$BENCHMARKS" != "[]" ]; then
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "bench_bitcoin (script verification)",
  "benchmark_suite": "script_verification",
  "benchmarks": $BENCHMARKS
}
EOF
    echo "✅ Results saved to: $OUTPUT_FILE"
else
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "No script verification benchmarks found or execution failed",
  "note": "Ensure Bitcoin Core is built with benchmarks and 'VerifyScriptBench', 'VerifyNestedIfScript' exist.",
  "benchmarks": []
}
EOF
    exit 1
fi

