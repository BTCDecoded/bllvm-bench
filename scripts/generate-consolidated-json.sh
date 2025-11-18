#!/bin/bash
# Generate Consolidated JSON Report
# Aggregates all benchmark JSON files into one final JSON output

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BLLVM_BENCH_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$SCRIPT_DIR/shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
OUTPUT_FILE="$OUTPUT_DIR/benchmark-results-consolidated-$TIMESTAMP.json"

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Generating Consolidated JSON Report                          ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""

# Find latest suite directory
LATEST_SUITE=$(find "$OUTPUT_DIR" -type d -name "suite-*" 2>/dev/null | sort | tail -1)

if [ -z "$LATEST_SUITE" ]; then
    echo "❌ No benchmark suite found. Run benchmarks first."
    exit 1
fi

echo "Using suite: $LATEST_SUITE"
echo ""

# Collect all JSON files
JSON_FILES=$(find "$OUTPUT_DIR" "$LATEST_SUITE" -name "*.json" -type f ! -name "summary.json" ! -name "*consolidated*" 2>/dev/null | sort)

if [ -z "$JSON_FILES" ]; then
    echo "❌ No benchmark JSON files found"
    exit 1
fi

echo "Found $(echo "$JSON_FILES" | wc -l) benchmark files"
echo ""

# Initialize consolidated JSON
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "suite_directory": "$LATEST_SUITE",
  "generated_by": "bllvm-bench consolidated JSON generator",
  "benchmarks": {},
  "summary": {
    "total_benchmarks": 0,
    "core_benchmarks": 0,
    "commons_benchmarks": 0,
    "comparisons": 0
  }
}
EOF

# Process each JSON file
BENCH_COUNT=0
CORE_COUNT=0
COMMONS_COUNT=0
COMPARISON_COUNT=0

while IFS= read -r json_file; do
    if [ ! -f "$json_file" ]; then
        continue
    fi
    
    BENCH_NAME=$(basename "$json_file" .json | sed 's/-[0-9]\{8\}-[0-9]\{6\}$//')
    
    # Extract benchmark data
    if echo "$BENCH_NAME" | grep -q "^core-"; then
        CORE_COUNT=$((CORE_COUNT + 1))
        BENCH_KEY=$(echo "$BENCH_NAME" | sed 's/^core-//')
        BENCH_COUNT=$((BENCH_COUNT + 1))
        
        # Add to consolidated JSON (initialize benchmark entry if it doesn't exist)
        jq --arg key "$BENCH_KEY" --argfile data "$json_file" \
           '.benchmarks[$key] = (.benchmarks[$key] // {}) | 
            .benchmarks[$key].name = $key |
            .benchmarks[$key].core = $data' \
           "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
        
    elif echo "$BENCH_NAME" | grep -q "^commons-"; then
        COMMONS_COUNT=$((COMMONS_COUNT + 1))
        BENCH_KEY=$(echo "$BENCH_NAME" | sed 's/^commons-//')
        # Only increment BENCH_COUNT if this is a new benchmark (not already counted from core)
        if ! jq -e ".benchmarks[\"$BENCH_KEY\"]" "$OUTPUT_FILE" >/dev/null 2>&1; then
            BENCH_COUNT=$((BENCH_COUNT + 1))
        fi
        
        # Add to consolidated JSON (initialize benchmark entry if it doesn't exist)
        jq --arg key "$BENCH_KEY" --argfile data "$json_file" \
           '.benchmarks[$key] = (.benchmarks[$key] // {}) | 
            .benchmarks[$key].name = $key |
            .benchmarks[$key].commons = $data' \
           "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
        
        # If both core and commons exist for same benchmark, it's a comparison
        if jq -e ".benchmarks[\"$BENCH_KEY\"].core" "$OUTPUT_FILE" >/dev/null 2>&1; then
            COMPARISON_COUNT=$((COMPARISON_COUNT + 1))
            
            # Calculate winner and speed difference with statistical analysis
            # Try multiple paths to extract timing data (different benchmarks have different structures)
            CORE_TIME=$(jq -r '
                .benchmarks["'"$BENCH_KEY"'"].core.bitcoin_core_block_validation.primary_comparison.time_per_block_ms //
                .benchmarks["'"$BENCH_KEY"'"].core.bitcoin_core_block_validation.connect_block_mixed_ecdsa_schnorr.time_per_block_ms //
                .benchmarks["'"$BENCH_KEY"'"].core.benchmarks[0].time_ms //
                .benchmarks["'"$BENCH_KEY"'"].core.benchmarks[0].time_per_block_ms //
                empty
            ' "$OUTPUT_FILE" 2>/dev/null || echo "")
            
            COMMONS_TIME=$(jq -r '
                .benchmarks["'"$BENCH_KEY"'"].commons.bitcoin_commons_block_validation.connect_block.time_per_block_ms //
                .benchmarks["'"$BENCH_KEY"'"].commons.benchmarks[0].time_ms //
                .benchmarks["'"$BENCH_KEY"'"].commons.benchmarks[0].time_per_block_ms //
                empty
            ' "$OUTPUT_FILE" 2>/dev/null || echo "")
            
            # Extract comprehensive statistical data
            # Try to extract from Criterion estimates.json for Commons
            COMMONS_STATS="null"
            if [ -f "$LATEST_SUITE/commons-$BENCH_KEY-bench" ] || [ -d "$BLLVM_BENCH_ROOT/target/criterion" ]; then
                # Try to find Criterion estimates.json
                CRITERION_DIR="$BLLVM_BENCH_ROOT/target/criterion"
                for bench_dir in "$CRITERION_DIR"/*; do
                    if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
                        COMMONS_STATS=$("$SCRIPT_DIR/shared/extract-criterion-stats.sh" "$bench_dir/base/estimates.json" 2>/dev/null || echo "null")
                        break
                    fi
                done
            fi
            
            # Extract from existing JSON if not found
            if [ "$COMMONS_STATS" = "null" ]; then
                COMMONS_STATS=$(jq -c '.benchmarks["'"$BENCH_KEY"'"].commons.benchmarks[0].statistics // .benchmarks["'"$BENCH_KEY"'"].commons.statistics // null' "$OUTPUT_FILE" 2>/dev/null || echo "null")
            fi
            
            # Extract from nanobench for Core (if available)
            CORE_STATS=$(jq -c '.benchmarks["'"$BENCH_KEY"'"].core.benchmarks[0].statistics // .benchmarks["'"$BENCH_KEY"'"].core.statistics // null' "$OUTPUT_FILE" 2>/dev/null || echo "null")
            
            # If still no time found, try to extract from any numeric field that looks like timing
            if [ -z "$CORE_TIME" ] || [ "$CORE_TIME" = "null" ] || [ "$CORE_TIME" = "0" ]; then
                CORE_TIME=$(jq -r '.benchmarks["'"$BENCH_KEY"'"].core | to_entries[] | select(.value | type == "number" and . > 0) | .value' "$OUTPUT_FILE" 2>/dev/null | head -1 || echo "")
            fi
            
            if [ -z "$COMMONS_TIME" ] || [ "$COMMONS_TIME" = "null" ] || [ "$COMMONS_TIME" = "0" ]; then
                COMMONS_TIME=$(jq -r '.benchmarks["'"$BENCH_KEY"'"].commons | to_entries[] | select(.value | type == "number" and . > 0) | .value' "$OUTPUT_FILE" 2>/dev/null | head -1 || echo "")
            fi
            
            if [ -n "$CORE_TIME" ] && [ -n "$COMMONS_TIME" ] && [ "$CORE_TIME" != "0" ] && [ "$CORE_TIME" != "null" ] && [ "$COMMONS_TIME" != "0" ] && [ "$COMMONS_TIME" != "null" ]; then
                if awk "BEGIN {exit !($CORE_TIME > $COMMONS_TIME)}" 2>/dev/null; then
                    WINNER="commons"
                    SPEEDUP=$(awk "BEGIN {printf \"%.2f\", $CORE_TIME / $COMMONS_TIME}" 2>/dev/null || echo "1")
                else
                    WINNER="core"
                    SPEEDUP=$(awk "BEGIN {printf \"%.2f\", $COMMONS_TIME / $CORE_TIME}" 2>/dev/null || echo "1")
                fi
                
                # Build comparison with statistics
                COMPARISON_JSON=$(jq -n \
                    --arg winner "$WINNER" \
                    --argjson speedup "$SPEEDUP" \
                    --argjson core_time "$CORE_TIME" \
                    --argjson commons_time "$COMMONS_TIME" \
                    --argjson core_stats "$CORE_STATS" \
                    --argjson commons_stats "$COMMONS_STATS" \
                    '{
                        winner: $winner,
                        speedup: $speedup,
                        core_time_ms: $core_time,
                        commons_time_ms: $commons_time,
                        core_statistics: $core_stats,
                        commons_statistics: $commons_stats
                    }')
                
                jq --arg key "$BENCH_KEY" --argjson comparison "$COMPARISON_JSON" \
                   '.benchmarks[$key].comparison = $comparison' \
                   "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
            fi
        fi
    fi
    
    BENCH_COUNT=$((BENCH_COUNT + 1))
done <<< "$JSON_FILES"

# Update summary
jq --argjson total "$BENCH_COUNT" \
   --argjson core "$CORE_COUNT" \
   --argjson commons "$COMMONS_COUNT" \
   --argjson comparisons "$COMPARISON_COUNT" \
   '.summary.total_benchmarks = $total | .summary.core_benchmarks = $core | .summary.commons_benchmarks = $commons | .summary.comparisons = $comparisons' \
   "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"

echo "✅ Consolidated JSON generated: $OUTPUT_FILE"

# Validate the output
if command -v "$SCRIPT_DIR/validate-benchmark.sh" >/dev/null 2>&1; then
    echo ""
    echo "Validating consolidated JSON..."
    "$SCRIPT_DIR/validate-benchmark.sh" "$OUTPUT_FILE" || echo "⚠️  Validation warnings (may be normal)"
fi

# Track history
if command -v "$SCRIPT_DIR/track-history.sh" >/dev/null 2>&1; then
    echo ""
    echo "Tracking history..."
    "$SCRIPT_DIR/track-history.sh" "$OUTPUT_FILE" || echo "⚠️  History tracking failed (non-fatal)"
fi
echo ""
echo "Summary:"
echo "  Total benchmarks: $BENCH_COUNT"
echo "  Core benchmarks: $CORE_COUNT"
echo "  Commons benchmarks: $COMMONS_COUNT"
echo "  Comparisons: $COMPARISON_COUNT"
echo ""
cat "$OUTPUT_FILE" | jq '.summary' 2>/dev/null || echo ""

