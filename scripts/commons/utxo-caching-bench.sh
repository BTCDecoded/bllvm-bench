#!/bin/bash
# Bitcoin Commons UTXO Caching Benchmark
# Measures UTXO caching performance using Criterion benchmarks
# Fair comparison with Core's CCoinsCaching benchmark

set -e

# Source common functions FIRST
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
BENCH_DIR="$BLLVM_BENCH_ROOT"
OUTPUT_FILE="$OUTPUT_DIR/commons-utxo-caching-bench-$(date +%Y%m%d-%H%M%S).json"

echo "=== Bitcoin Commons UTXO Caching Benchmark ==="
echo ""

cd "$BENCH_DIR"

echo "Running UTXO caching benchmark (this may take 1-2 minutes)..."
echo "This benchmarks UTXO insert/get/remove operations (matches Core's CCoinsCaching)."

# Run the performance_focused benchmark which includes UTXO operations
BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench utxo_commitments --features production 2>&1 || echo "")

# Extract UTXO operation results - look for "utxo/insert", "utxo/get", "utxo/remove"
UTXO_INSERT_LINE=$(echo "$BENCH_OUTPUT" | grep -A 5 "utxo/insert\|utxo.*insert" | grep -i "time:" | head -1 || echo "")
UTXO_GET_LINE=$(echo "$BENCH_OUTPUT" | grep -A 5 "utxo/get\|utxo.*get" | grep -i "time:" | head -1 || echo "")
UTXO_REMOVE_LINE=$(echo "$BENCH_OUTPUT" | grep -A 5 "utxo/remove\|utxo.*remove" | grep -i "time:" | head -1 || echo "")

# Parse Criterion output
parse_criterion_time() {
    local line="$1"
    if [ -z "$line" ]; then
        echo "0|ns"
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
    echo "${median}|${unit}"
}

UTXO_INSERT_DATA=$(parse_criterion_time "$UTXO_INSERT_LINE")
UTXO_GET_DATA=$(parse_criterion_time "$UTXO_GET_LINE")
UTXO_REMOVE_DATA=$(parse_criterion_time "$UTXO_REMOVE_LINE")

# Convert to nanoseconds
UTXO_INSERT_NS="0"
UTXO_GET_NS="0"
UTXO_REMOVE_NS="0"

for op in "INSERT" "GET" "REMOVE"; do
    case "$op" in
        "INSERT") DATA="$UTXO_INSERT_DATA" ;;
        "GET") DATA="$UTXO_GET_DATA" ;;
        "REMOVE") DATA="$UTXO_REMOVE_DATA" ;;
    esac
    
    if [ -n "$DATA" ] && [ "$DATA" != "0|ns" ]; then
        median=$(echo "$DATA" | cut -d'|' -f1)
        unit=$(echo "$DATA" | cut -d'|' -f2)
        if [ "$unit" = "ns" ]; then
            eval "UTXO_${op}_NS=\$(echo \"\$median\" | awk '{printf \"%.0f\", \$1}' 2>/dev/null || echo \"0\")"
        elif [ "$unit" = "us" ] || [ "$unit" = "µs" ]; then
            eval "UTXO_${op}_NS=\$(echo \"\$median\" | awk '{printf \"%.0f\", \$1 * 1000}' 2>/dev/null || echo \"0\")"
        fi
    fi
done

# Convert to milliseconds
UTXO_INSERT_MS=$(awk "BEGIN {printf \"%.6f\", $UTXO_INSERT_NS / 1000000}" 2>/dev/null || echo "0")
UTXO_GET_MS=$(awk "BEGIN {printf \"%.6f\", $UTXO_GET_NS / 1000000}" 2>/dev/null || echo "0")
UTXO_REMOVE_MS=$(awk "BEGIN {printf \"%.6f\", $UTXO_REMOVE_NS / 1000000}" 2>/dev/null || echo "0")

# Calculate ops per second
UTXO_INSERT_OPS="0"
UTXO_GET_OPS="0"
UTXO_REMOVE_OPS="0"

if [ "$UTXO_INSERT_NS" != "0" ] && [ -n "$UTXO_INSERT_NS" ]; then
    UTXO_INSERT_OPS=$(awk "BEGIN {if ($UTXO_INSERT_NS > 0) printf \"%.0f\", 1000000000 / $UTXO_INSERT_NS; else print \"0\"}" 2>/dev/null || echo "0")
fi
if [ "$UTXO_GET_NS" != "0" ] && [ -n "$UTXO_GET_NS" ]; then
    UTXO_GET_OPS=$(awk "BEGIN {if ($UTXO_GET_NS > 0) printf \"%.0f\", 1000000000 / $UTXO_GET_NS; else print \"0\"}" 2>/dev/null || echo "0")
fi
if [ "$UTXO_REMOVE_NS" != "0" ] && [ -n "$UTXO_REMOVE_NS" ]; then
    UTXO_REMOVE_OPS=$(awk "BEGIN {if ($UTXO_REMOVE_NS > 0) printf \"%.0f\", 1000000000 / $UTXO_REMOVE_NS; else print \"0\"}" 2>/dev/null || echo "0")
fi

# Fallback: Try parsing from Criterion JSON output
if [ "$UTXO_INSERT_NS" = "0" ] || [ "$UTXO_GET_NS" = "0" ] || [ "$UTXO_REMOVE_NS" = "0" ]; then
    CRITERION_DIR="$BENCH_DIR/target/criterion"
    if [ -d "$CRITERION_DIR" ]; then
        for bench_dir in "$CRITERION_DIR"/utxo/*; do
            if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
                BENCH_NAME=$(basename "$bench_dir")
                TIME_NS=$(jq -r '.mean.point_estimate' "$bench_dir/base/estimates.json" 2>/dev/null || echo "")
                if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
                    TIME_MS=$(awk "BEGIN {printf \"%.9f\", $TIME_NS / 1000000}" 2>/dev/null || echo "0")
                    if echo "$BENCH_NAME" | grep -qi "insert"; then
                        UTXO_INSERT_NS=$(printf "%.0f" "$TIME_NS" 2>/dev/null || echo "0")
                        UTXO_INSERT_MS="$TIME_MS"
                    elif echo "$BENCH_NAME" | grep -qi "get"; then
                        UTXO_GET_NS=$(printf "%.0f" "$TIME_NS" 2>/dev/null || echo "0")
                        UTXO_GET_MS="$TIME_MS"
                    elif echo "$BENCH_NAME" | grep -qi "remove"; then
                        UTXO_REMOVE_NS=$(printf "%.0f" "$TIME_NS" 2>/dev/null || echo "0")
                        UTXO_REMOVE_MS="$TIME_MS"
                    fi
                fi
            fi
        done
    fi
fi

BENCHMARKS="[]"

if [ "$UTXO_INSERT_NS" != "0" ] && [ -n "$UTXO_INSERT_NS" ]; then
    UTXO_INSERT_OPS=$(awk "BEGIN {if ($UTXO_INSERT_NS > 0) printf \"%.0f\", 1000000000 / $UTXO_INSERT_NS; else print \"0\"}" 2>/dev/null || echo "0")
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "insert" --argjson time_ms "$UTXO_INSERT_MS" --argjson time_ns "$UTXO_INSERT_NS" --argjson ops "$UTXO_INSERT_OPS" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns, "ops_per_sec": $ops}]' 2>/dev/null || echo "$BENCHMARKS")
fi

if [ "$UTXO_GET_NS" != "0" ] && [ -n "$UTXO_GET_NS" ]; then
    UTXO_GET_OPS=$(awk "BEGIN {if ($UTXO_GET_NS > 0) printf \"%.0f\", 1000000000 / $UTXO_GET_NS; else print \"0\"}" 2>/dev/null || echo "0")
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "get" --argjson time_ms "$UTXO_GET_MS" --argjson time_ns "$UTXO_GET_NS" --argjson ops "$UTXO_GET_OPS" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns, "ops_per_sec": $ops}]' 2>/dev/null || echo "$BENCHMARKS")
fi

if [ "$UTXO_REMOVE_NS" != "0" ] && [ -n "$UTXO_REMOVE_NS" ]; then
    UTXO_REMOVE_OPS=$(awk "BEGIN {if ($UTXO_REMOVE_NS > 0) printf \"%.0f\", 1000000000 / $UTXO_REMOVE_NS; else print \"0\"}" 2>/dev/null || echo "0")
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "remove" --argjson time_ms "$UTXO_REMOVE_MS" --argjson time_ns "$UTXO_REMOVE_NS" --argjson ops "$UTXO_REMOVE_OPS" '. += [{"name": $name, "time_ms": $time_ms, "time_ns": $time_ns, "ops_per_sec": $ops}]' 2>/dev/null || echo "$BENCHMARKS")
fi

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmark (UTXO caching operations - matches Core's CCoinsCaching)",
  "benchmarks": $BENCHMARKS
}
EOF

echo "✅ Results saved to: $OUTPUT_FILE"
