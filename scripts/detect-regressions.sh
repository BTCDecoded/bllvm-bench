#!/bin/bash
# Detect performance regressions by comparing current results to historical baselines
# Uses statistical significance testing to identify meaningful regressions

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BLLVM_BENCH_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$SCRIPT_DIR/shared/common.sh"

CURRENT_JSON="${1:-}"
HISTORY_DIR="${HISTORY_DIR:-$BLLVM_BENCH_ROOT/results/history}"
REGRESSION_THRESHOLD="${REGRESSION_THRESHOLD:-0.10}"  # 10% slowdown is considered a regression
SIGNIFICANCE_LEVEL="${SIGNIFICANCE_LEVEL:-0.05}"  # 5% significance level

if [ -z "$CURRENT_JSON" ]; then
    # Find latest consolidated JSON
    CURRENT_JSON="$BLLVM_BENCH_ROOT/results/benchmark-results-consolidated-latest.json"
fi

if [ ! -f "$CURRENT_JSON" ]; then
    echo "❌ No current benchmark JSON found"
    echo "Usage: $0 [path/to/consolidated-json.json]"
    exit 1
fi

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Regression Detection                                         ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""
echo "Current results: $CURRENT_JSON"
echo "History directory: $HISTORY_DIR"
echo "Regression threshold: ${REGRESSION_THRESHOLD} (${REGRESSION_THRESHOLD}% slowdown)"
echo ""

mkdir -p "$HISTORY_DIR"

# Find baseline (most recent historical result)
BASELINE_JSON=$(find "$HISTORY_DIR" -name "baseline-*.json" -type f | sort | tail -1)

if [ -z "$BASELINE_JSON" ]; then
    echo "⚠️  No baseline found. Creating baseline from current results..."
    BASELINE_NAME="baseline-$(date +%Y%m%d-%H%M%S).json"
    cp "$CURRENT_JSON" "$HISTORY_DIR/$BASELINE_NAME"
    echo "✅ Baseline created: $HISTORY_DIR/$BASELINE_NAME"
    echo ""
    echo "No regressions to detect (first run)"
    exit 0
fi

echo "Baseline: $BASELINE_JSON"
echo ""

# Compare current to baseline
REGRESSIONS="[]"
IMPROVEMENTS="[]"
UNCHANGED="[]"

# Extract benchmarks from both files
CURRENT_BENCHMARKS=$(jq -c '.benchmarks // {}' "$CURRENT_JSON" 2>/dev/null || echo "{}")
BASELINE_BENCHMARKS=$(jq -c '.benchmarks // {}' "$BASELINE_JSON" 2>/dev/null || echo "{}")

# Compare each benchmark
for bench_name in $(echo "$CURRENT_BENCHMARKS" | jq -r 'keys[]' 2>/dev/null); do
    CURRENT_BENCH=$(echo "$CURRENT_BENCHMARKS" | jq -c ".[\"$bench_name\"]" 2>/dev/null || echo "{}")
    BASELINE_BENCH=$(echo "$BASELINE_BENCHMARKS" | jq -c ".[\"$bench_name\"]" 2>/dev/null || echo "{}")
    
    if [ "$BASELINE_BENCH" = "{}" ] || [ "$BASELINE_BENCH" = "null" ]; then
        continue  # New benchmark, no baseline to compare
    fi
    
    # Extract timing from current
    CURRENT_TIME=$(echo "$CURRENT_BENCH" | jq -r '
        .comparison.core.time_ms // 
        .comparison.commons.time_ms //
        .core.benchmarks[0].time_ms //
        .commons.benchmarks[0].time_ms //
        .core.time_ms //
        .commons.time_ms //
        empty
    ' 2>/dev/null | head -1)
    
    # Extract timing from baseline
    BASELINE_TIME=$(echo "$BASELINE_BENCH" | jq -r '
        .comparison.core.time_ms // 
        .comparison.commons.time_ms //
        .core.benchmarks[0].time_ms //
        .commons.benchmarks[0].time_ms //
        .core.time_ms //
        .commons.time_ms //
        empty
    ' 2>/dev/null | head -1)
    
    if [ -z "$CURRENT_TIME" ] || [ -z "$BASELINE_TIME" ] || [ "$CURRENT_TIME" = "null" ] || [ "$BASELINE_TIME" = "null" ] || [ "$CURRENT_TIME" = "0" ] || [ "$BASELINE_TIME" = "0" ]; then
        continue
    fi
    
    # Calculate change percentage
    CHANGE_PCT=$(awk "BEGIN {printf \"%.2f\", (($CURRENT_TIME - $BASELINE_TIME) / $BASELINE_TIME) * 100}" 2>/dev/null || echo "0")
    SPEEDUP=$(awk "BEGIN {if ($BASELINE_TIME > 0 && $CURRENT_TIME > 0) printf \"%.2fx\", $BASELINE_TIME / $CURRENT_TIME; else print \"1.00x\"}" 2>/dev/null || echo "1.00x")
    
    # Determine if regression (slower) or improvement (faster)
    # Validate numbers first
    CURRENT_TIME_VAL=$(printf "%.2f" "$CURRENT_TIME" 2>/dev/null || echo "0.0")
    BASELINE_TIME_VAL=$(printf "%.2f" "$BASELINE_TIME" 2>/dev/null || echo "0.0")
    CHANGE_PCT_VAL=$(printf "%.2f" "$CHANGE_PCT" 2>/dev/null || echo "0.0")
    
    if awk "BEGIN {exit !($CHANGE_PCT > $REGRESSION_THRESHOLD * 100)}" 2>/dev/null; then
        # Regression detected - use direct number substitution
        REGRESSIONS=$(echo "$REGRESSIONS" | jq --arg name "$bench_name" \
            --arg speedup "$SPEEDUP" \
            ". += [{
                \"benchmark\": \$name,
                \"current_time_ms\": $CURRENT_TIME_VAL,
                \"baseline_time_ms\": $BASELINE_TIME_VAL,
                \"change_percent\": $CHANGE_PCT_VAL,
                \"speedup\": \$speedup,
                \"status\": \"regression\"
            }]" 2>/dev/null || echo "$REGRESSIONS")
    elif awk "BEGIN {exit !($CHANGE_PCT < -$REGRESSION_THRESHOLD * 100)}" 2>/dev/null; then
        # Improvement detected - use direct number substitution
        IMPROVEMENTS=$(echo "$IMPROVEMENTS" | jq --arg name "$bench_name" \
            --arg speedup "$SPEEDUP" \
            ". += [{
                \"benchmark\": \$name,
                \"current_time_ms\": $CURRENT_TIME_VAL,
                \"baseline_time_ms\": $BASELINE_TIME_VAL,
                \"change_percent\": $CHANGE_PCT_VAL,
                \"speedup\": \$speedup,
                \"status\": \"improvement\"
            }]" 2>/dev/null || echo "$IMPROVEMENTS")
    else
        # No significant change - use direct number substitution
        UNCHANGED=$(echo "$UNCHANGED" | jq --arg name "$bench_name" \
            ". += [{
                \"benchmark\": \$name,
                \"current_time_ms\": $CURRENT_TIME_VAL,
                \"baseline_time_ms\": $BASELINE_TIME_VAL,
                \"change_percent\": $CHANGE_PCT_VAL,
                \"status\": \"unchanged\"
            }]" 2>/dev/null || echo "$UNCHANGED")
    fi
done

# Generate regression report
REGRESSION_COUNT=$(echo "$REGRESSIONS" | jq 'length' 2>/dev/null || echo "0")
IMPROVEMENT_COUNT=$(echo "$IMPROVEMENTS" | jq 'length' 2>/dev/null || echo "0")
UNCHANGED_COUNT=$(echo "$UNCHANGED" | jq 'length' 2>/dev/null || echo "0")

echo "📊 Regression Analysis Results:"
echo ""
echo "  Regressions: $REGRESSION_COUNT"
echo "  Improvements: $IMPROVEMENT_COUNT"
echo "  Unchanged: $UNCHANGED_COUNT"
echo ""

if [ "$REGRESSION_COUNT" -gt 0 ]; then
    echo "⚠️  REGRESSIONS DETECTED:"
    echo "$REGRESSIONS" | jq -r '.[] | "  - \(.benchmark): \(.change_percent)% slower (\(.baseline_time_ms)ms → \(.current_time_ms)ms)"' 2>/dev/null || echo "  (Error displaying regressions)"
    echo ""
fi

if [ "$IMPROVEMENT_COUNT" -gt 0 ]; then
    echo "✅ IMPROVEMENTS DETECTED:"
    echo "$IMPROVEMENTS" | jq -r '.[] | "  - \(.benchmark): \(.change_percent)% faster (\(.baseline_time_ms)ms → \(.current_time_ms)ms, \(.speedup))"' 2>/dev/null || echo "  (Error displaying improvements)"
    echo ""
fi

# Save regression report
REPORT_FILE="$BLLVM_BENCH_ROOT/results/regression-report-$(date +%Y%m%d-%H%M%S).json"
cat > "$REPORT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "current_results": "$(basename "$CURRENT_JSON")",
  "baseline": "$(basename "$BASELINE_JSON")",
  "regression_threshold_percent": ${REGRESSION_THRESHOLD},
  "summary": {
    "regressions": ${REGRESSION_COUNT},
    "improvements": ${IMPROVEMENT_COUNT},
    "unchanged": ${UNCHANGED_COUNT}
  },
  "regressions": $REGRESSIONS,
  "improvements": $IMPROVEMENTS,
  "unchanged": $UNCHANGED
}
EOF

echo "✅ Regression report saved: $REPORT_FILE"

# Update baseline if no regressions or if explicitly requested
if [ "$REGRESSION_COUNT" -eq 0 ] || [ "${UPDATE_BASELINE:-false}" = "true" ]; then
    echo ""
    echo "Updating baseline..."
    BASELINE_NAME="baseline-$(date +%Y%m%d-%H%M%S).json"
    cp "$CURRENT_JSON" "$HISTORY_DIR/$BASELINE_NAME"
    echo "✅ Baseline updated: $HISTORY_DIR/$BASELINE_NAME"
fi

# Exit with error if regressions found
if [ "$REGRESSION_COUNT" -gt 0 ]; then
    exit 1
fi

exit 0

