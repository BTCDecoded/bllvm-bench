#!/bin/bash
# Generate Consolidated JSON Report
# Aggregates all benchmark JSON files into one final JSON output

# Don't exit on error - we want to see debug output even if some files fail
set +e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BLLVM_BENCH_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$SCRIPT_DIR/shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/benchmark-results-consolidated-latest.json"

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Generating Consolidated JSON Report                          ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""

# Find ALL suite directories (not just latest - we want full coverage)
ALL_SUITES=$(find "$OUTPUT_DIR" -type d -name "suite-*" 2>/dev/null | sort)
LATEST_SUITE=$(echo "$ALL_SUITES" | tail -1)

if [ -z "$LATEST_SUITE" ]; then
    echo "⚠️  No benchmark suite found, searching all result directories..."
    LATEST_SUITE="$OUTPUT_DIR"
fi

echo "Using suite: $LATEST_SUITE"
if [ -n "$ALL_SUITES" ] && [ $(echo "$ALL_SUITES" | wc -l) -gt 1 ]; then
    echo "Found $(echo "$ALL_SUITES" | wc -l) suite directories - will search all for maximum coverage"
fi
echo ""

# Collect ALL JSON files from ALL suites and results root for maximum coverage
# Exclude history files, trends, and other non-benchmark files
SEARCH_DIRS="$OUTPUT_DIR"
if [ -n "$ALL_SUITES" ]; then
    SEARCH_DIRS="$SEARCH_DIRS $ALL_SUITES"
fi

# First, find ALL JSON files (for debugging)
ALL_JSON_FILES=$(find $SEARCH_DIRS -name "*.json" -type f \
    ! -name "summary.json" \
    ! -name "*consolidated*" \
    ! -name "history-*.json" \
    ! -name "trends-*.json" \
    ! -name "timeseries.json" \
    ! -path "*/history/*" \
    2>/dev/null | sort | uniq)

# Filter for benchmark files (core- or commons- prefix)
JSON_FILES=$(echo "$ALL_JSON_FILES" | grep -E "(core-|commons-)" || echo "")

# Debug: Show what we found
TOTAL_JSON_COUNT=$(echo "$ALL_JSON_FILES" | grep -v '^$' | wc -l)
BENCH_JSON_COUNT=$(echo "$JSON_FILES" | grep -v '^$' | wc -l)
echo "Total JSON files found: $TOTAL_JSON_COUNT" >&2
echo "Benchmark JSON files (core-|commons-): $BENCH_JSON_COUNT" >&2

# Show breakdown
CORE_JSON_COUNT=$(echo "$JSON_FILES" | grep -c "^.*core-" 2>/dev/null || echo "0")
COMMONS_JSON_COUNT=$(echo "$JSON_FILES" | grep -c "^.*commons-" 2>/dev/null || echo "0")
echo "  Core JSON files: $CORE_JSON_COUNT" >&2
echo "  Commons JSON files: $COMMONS_JSON_COUNT" >&2

if [ -z "$JSON_FILES" ] || [ "$BENCH_JSON_COUNT" -eq 0 ]; then
    echo "⚠️  No benchmark JSON files found! Listing all JSON files:" >&2
    echo "$ALL_JSON_FILES" | head -30 | sed 's/^/  /' >&2
    echo "" >&2
    echo "Searching for any files with 'core' or 'commons' in name:" >&2
    find $SEARCH_DIRS -name "*core*" -o -name "*commons*" 2>/dev/null | grep -E "\.json$" | head -20 | sed 's/^/  /' >&2
fi

if [ -z "$JSON_FILES" ]; then
    echo "❌ No benchmark JSON files found"
    echo "   Searched in: $SEARCH_DIRS"
    echo "   Listing all JSON files found:"
    find $SEARCH_DIRS -name "*.json" -type f 2>/dev/null | head -20 || echo "   (none found)"
    exit 1
fi

# Remove the old echo statement - we already have better debug output above
CORE_FILE_COUNT=$(echo "$JSON_FILES" | grep -c "core-" 2>/dev/null || echo "0")
COMMONS_FILE_COUNT=$(echo "$JSON_FILES" | grep -c "commons-" 2>/dev/null || echo "0")
echo "Core files: $CORE_FILE_COUNT" >&2
echo "Commons files: $COMMONS_FILE_COUNT" >&2
echo "" >&2

# Debug: Show first few files of each type (to stderr so it's always visible)
if [ "$CORE_FILE_COUNT" -gt 0 ]; then
    echo "Sample Core files:" >&2
    echo "$JSON_FILES" | grep "core-" | head -5 | sed 's/^/  /' >&2
else
    echo "⚠️  No Core JSON files found!" >&2
    echo "   Searching for any files with 'core' in name:" >&2
    find $SEARCH_DIRS -name "*core*" -type f 2>/dev/null | head -10 | sed 's/^/  /' >&2 || echo "   (none found)" >&2
    echo "   All JSON files found:" >&2
    find $SEARCH_DIRS -name "*.json" -type f 2>/dev/null | head -20 | sed 's/^/  /' >&2
fi
if [ "$COMMONS_FILE_COUNT" -gt 0 ]; then
    echo "Sample Commons files:" >&2
    echo "$JSON_FILES" | grep "commons-" | head -5 | sed 's/^/  /' >&2
fi
echo "" >&2

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
        echo "⚠️  Skipping non-existent file: $json_file"
        continue
    fi
    
    BENCH_NAME=$(basename "$json_file" .json | sed 's/-[0-9]\{8\}-[0-9]\{6\}$//')
    
    # Debug: Show what we're processing (to stderr)
    if echo "$BENCH_NAME" | grep -q "^core-"; then
        echo "Processing Core file: $json_file (bench_name: $BENCH_NAME)" >&2
    fi
    
    # Handle special combined benchmarks (RPC, Concurrent, Memory, Parallel)
    if echo "$BENCH_NAME" | grep -qE "^(performance-rpc-http|concurrent-operations-fair|memory-efficiency-fair|parallel-block-validation-bench)$"; then
        # These benchmarks contain both Core and Commons data in one file
        BENCH_KEY=$(echo "$BENCH_NAME" | sed 's/-fair$//' | sed 's/performance-rpc-http/rpc-performance/' | sed 's/parallel-block-validation-bench/parallel-block-validation/')
        BENCH_COUNT=$((BENCH_COUNT + 1))
        
        # Read JSON file - it should have both core and commons data
        DATA_CONTENT=$(cat "$json_file" 2>/dev/null || echo "{}")
        
        # Validate JSON first
        if ! echo "$DATA_CONTENT" | jq . >/dev/null 2>&1; then
            echo "  ⚠️  Warning: Invalid JSON in $json_file, skipping" >&2
            continue
        fi
        
        # Try to extract core and commons data from the combined file
        # Handle nested structures like concurrent_operations_fair.bitcoin_core
        CORE_DATA=$(echo "$DATA_CONTENT" | jq '.core // .bitcoin_core // .concurrent_operations_fair.bitcoin_core // .rpc_performance.bitcoin_core // .memory_efficiency_fair.bitcoin_core // {}' 2>/dev/null || echo "{}")
        COMMONS_DATA=$(echo "$DATA_CONTENT" | jq '.commons // .bitcoin_commons // .concurrent_operations_fair.bitcoin_commons // .rpc_performance.bitcoin_commons // .memory_efficiency_fair.bitcoin_commons // {}' 2>/dev/null || echo "{}")
        
        # Check if data is actually non-empty using jq (more reliable than string comparison)
        CORE_IS_EMPTY=true
        COMMONS_IS_EMPTY=true
        
        if echo "$CORE_DATA" | jq -e 'keys | length > 0' >/dev/null 2>&1; then
            CORE_IS_EMPTY=false
            echo "  Found Core data in $BENCH_KEY" >&2
        fi
        
        if echo "$COMMONS_DATA" | jq -e 'keys | length > 0' >/dev/null 2>&1; then
            COMMONS_IS_EMPTY=false
            echo "  Found Commons data in $BENCH_KEY" >&2
        fi
        
        # If the file structure is different, use the whole file for both
        if [ "$CORE_IS_EMPTY" = "true" ] && [ "$COMMONS_IS_EMPTY" = "true" ]; then
            # File might have a different structure - use it as-is and let comparison logic handle it
            # Use temp file to avoid shell escaping issues
            TEMP_JSON=$(mktemp)
            echo "$DATA_CONTENT" > "$TEMP_JSON"
            if ! jq . "$TEMP_JSON" >/dev/null 2>&1; then
                echo "{}" > "$TEMP_JSON"
            fi
            jq --arg key "$BENCH_KEY" --slurpfile data "$TEMP_JSON" \
               '.benchmarks[$key] = (.benchmarks[$key] // {}) | 
                .benchmarks[$key].name = $key |
                .benchmarks[$key].combined = $data[0]' \
               "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
            rm -f "$TEMP_JSON"
            
            # Try to split if possible (even if both are empty, check original structure)
            if echo "$DATA_CONTENT" | jq -e '.core // .bitcoin_core // .concurrent_operations_fair.bitcoin_core // empty' >/dev/null 2>&1; then
                if [ "$CORE_IS_EMPTY" = "false" ]; then
                    CORE_COUNT=$((CORE_COUNT + 1))
                    echo "  Counted Core benchmark from $BENCH_KEY" >&2
                fi
                TEMP_JSON=$(mktemp)
                echo "$CORE_DATA" > "$TEMP_JSON"
                if jq . "$TEMP_JSON" >/dev/null 2>&1; then
                    jq --arg key "$BENCH_KEY" --slurpfile data "$TEMP_JSON" \
                       '.benchmarks[$key].core = $data[0]' \
                       "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
                fi
                rm -f "$TEMP_JSON"
            fi
            if echo "$DATA_CONTENT" | jq -e '.commons // .bitcoin_commons // .concurrent_operations_fair.bitcoin_commons // empty' >/dev/null 2>&1; then
                if [ "$COMMONS_IS_EMPTY" = "false" ]; then
                    COMMONS_COUNT=$((COMMONS_COUNT + 1))
                    echo "  Counted Commons benchmark from $BENCH_KEY" >&2
                fi
                TEMP_JSON=$(mktemp)
                echo "$COMMONS_DATA" > "$TEMP_JSON"
                if jq . "$TEMP_JSON" >/dev/null 2>&1; then
                    jq --arg key "$BENCH_KEY" --slurpfile data "$TEMP_JSON" \
                       '.benchmarks[$key].commons = $data[0]' \
                       "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
                fi
                rm -f "$TEMP_JSON"
            fi
        else
            # File has separate core and commons data
            # Use temp files to avoid shell escaping issues
            TEMP_CORE=$(mktemp)
            TEMP_COMMONS=$(mktemp)
            echo "$CORE_DATA" > "$TEMP_CORE"
            echo "$COMMONS_DATA" > "$TEMP_COMMONS"
            if jq . "$TEMP_CORE" >/dev/null 2>&1 && jq . "$TEMP_COMMONS" >/dev/null 2>&1; then
                jq --arg key "$BENCH_KEY" --slurpfile core_data "$TEMP_CORE" --slurpfile commons_data "$TEMP_COMMONS" \
                   '.benchmarks[$key] = (.benchmarks[$key] // {}) | 
                    .benchmarks[$key].name = $key |
                    .benchmarks[$key].core = $core_data[0] |
                    .benchmarks[$key].commons = $commons_data[0]' \
                   "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
            fi
            rm -f "$TEMP_CORE" "$TEMP_COMMONS"
            
            # Count Core and Commons benchmarks if they have data
            # Check if data is non-empty (not just "{}")
            CORE_HAS_DATA=false
            COMMONS_HAS_DATA=false
            
            # Check if CORE_DATA has actual content (not just empty object)
            if [ "$CORE_DATA" != "{}" ] && [ -n "$CORE_DATA" ]; then
                # Use jq to check if object has any keys
                if echo "$CORE_DATA" | jq -e 'keys | length > 0' >/dev/null 2>&1; then
                    CORE_HAS_DATA=true
                    CORE_COUNT=$((CORE_COUNT + 1))
                    echo "  Found Core data in $BENCH_KEY" >&2
                fi
            fi
            
            # Check if COMMONS_DATA has actual content
            if [ "$COMMONS_DATA" != "{}" ] && [ -n "$COMMONS_DATA" ]; then
                # Use jq to check if object has any keys
                if echo "$COMMONS_DATA" | jq -e 'keys | length > 0' >/dev/null 2>&1; then
                    COMMONS_HAS_DATA=true
                    COMMONS_COUNT=$((COMMONS_COUNT + 1))
                    echo "  Found Commons data in $BENCH_KEY" >&2
                fi
            fi
            
            # Count as comparison if both have data
            if [ "$CORE_HAS_DATA" = "true" ] && [ "$COMMONS_HAS_DATA" = "true" ]; then
                COMPARISON_COUNT=$((COMPARISON_COUNT + 1))
                echo "  Found comparison data in $BENCH_KEY" >&2
            fi
        fi
        continue
    fi
    
    # Extract benchmark data
    if echo "$BENCH_NAME" | grep -q "^core-"; then
        CORE_COUNT=$((CORE_COUNT + 1))
        BENCH_KEY=$(echo "$BENCH_NAME" | sed 's/^core-//')
        BENCH_COUNT=$((BENCH_COUNT + 1))
        
        echo "  Adding Core benchmark: $BENCH_KEY from $json_file" >&2
        
        # Add to consolidated JSON (initialize benchmark entry if it doesn't exist)
        # Read JSON file content and validate it
        DATA_CONTENT=$(cat "$json_file" 2>/dev/null || echo "{}")
        if [ "$DATA_CONTENT" = "{}" ] || [ -z "$DATA_CONTENT" ]; then
            echo "  ⚠️  Warning: File is empty or unreadable: $json_file" >&2
            DATA_CONTENT="{}"
        fi
        
        # Validate JSON before passing to jq
        # Use a temp file to avoid shell escaping issues
        TEMP_JSON=$(mktemp)
        echo "$DATA_CONTENT" > "$TEMP_JSON"
        
        if ! jq . "$TEMP_JSON" >/dev/null 2>&1; then
            echo "  ⚠️  Warning: Invalid JSON in $json_file, using empty object" >&2
            echo "{}" > "$TEMP_JSON"
        fi
        
        # Use jq to safely merge the data (read from temp file to avoid shell escaping)
        # IMPORTANT: Preserve existing commons data if it exists
        if ! jq --arg key "$BENCH_KEY" --slurpfile data "$TEMP_JSON" \
           '.benchmarks[$key] = (.benchmarks[$key] // {}) | 
            .benchmarks[$key].name = $key |
            .benchmarks[$key].core = $data[0] |
            .benchmarks[$key].commons = (.benchmarks[$key].commons // null)' \
           "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" 2>/dev/null; then
            echo "  ⚠️  Warning: Failed to merge JSON for $BENCH_KEY" >&2
            # Try with empty object
            echo "{}" > "$TEMP_JSON"
            jq --arg key "$BENCH_KEY" --slurpfile data "$TEMP_JSON" \
               '.benchmarks[$key] = (.benchmarks[$key] // {}) | 
                .benchmarks[$key].name = $key |
                .benchmarks[$key].core = $data[0] |
                .benchmarks[$key].commons = (.benchmarks[$key].commons // null)' \
               "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" 2>/dev/null || true
        fi
        rm -f "$TEMP_JSON"
        mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE" 2>/dev/null || true
        
    elif echo "$BENCH_NAME" | grep -q "^commons-"; then
        COMMONS_COUNT=$((COMMONS_COUNT + 1))
        BENCH_KEY=$(echo "$BENCH_NAME" | sed 's/^commons-//')
        # Only increment BENCH_COUNT if this is a new benchmark (not already counted from core)
        if ! jq -e ".benchmarks[\"$BENCH_KEY\"]" "$OUTPUT_FILE" >/dev/null 2>&1; then
            BENCH_COUNT=$((BENCH_COUNT + 1))
        fi
        
        echo "  Adding Commons benchmark: $BENCH_KEY from $json_file" >&2
        
        # Add to consolidated JSON (initialize benchmark entry if it doesn't exist)
        # Read JSON file content and validate it
        DATA_CONTENT=$(cat "$json_file" 2>/dev/null || echo "{}")
        if [ "$DATA_CONTENT" = "{}" ] || [ -z "$DATA_CONTENT" ]; then
            echo "  ⚠️  Warning: File is empty or unreadable: $json_file" >&2
            DATA_CONTENT="{}"
        fi
        
        # Validate JSON before passing to jq
        # Use a temp file to avoid shell escaping issues
        TEMP_JSON=$(mktemp)
        echo "$DATA_CONTENT" > "$TEMP_JSON"
        
        if ! jq . "$TEMP_JSON" >/dev/null 2>&1; then
            echo "  ⚠️  Warning: Invalid JSON in $json_file, using empty object" >&2
            echo "{}" > "$TEMP_JSON"
        fi
        
        # Use jq to safely merge the data (read from temp file to avoid shell escaping)
        # IMPORTANT: Preserve existing core data if it exists
        if ! jq --arg key "$BENCH_KEY" --slurpfile data "$TEMP_JSON" \
           '.benchmarks[$key] = (.benchmarks[$key] // {}) | 
            .benchmarks[$key].name = $key |
            .benchmarks[$key].commons = $data[0] |
            .benchmarks[$key].core = (.benchmarks[$key].core // null)' \
           "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" 2>/dev/null; then
            echo "  ⚠️  Warning: Failed to merge JSON for $BENCH_KEY" >&2
            # Try with empty object
            echo "{}" > "$TEMP_JSON"
            jq --arg key "$BENCH_KEY" --slurpfile data "$TEMP_JSON" \
               '.benchmarks[$key] = (.benchmarks[$key] // {}) | 
                .benchmarks[$key].name = $key |
                .benchmarks[$key].commons = $data[0] |
                .benchmarks[$key].core = (.benchmarks[$key].core // null)' \
               "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" 2>/dev/null || true
        fi
        rm -f "$TEMP_JSON"
        mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE" 2>/dev/null || true
        
        # If both core and commons exist for same benchmark, it's a comparison
        if jq -e ".benchmarks[\"$BENCH_KEY\"].core" "$OUTPUT_FILE" >/dev/null 2>&1; then
            COMPARISON_COUNT=$((COMPARISON_COUNT + 1))
            
            # Calculate winner and speed difference with statistical analysis
            # Try multiple paths to extract timing data (different benchmarks have different structures)
            # Comprehensive extraction for Core - try all possible paths
            CORE_TIME=$(jq -r '
                .benchmarks["'"$BENCH_KEY"'"].core.bitcoin_core_block_validation.primary_comparison.time_per_block_ms //
                .benchmarks["'"$BENCH_KEY"'"].core.bitcoin_core_block_validation.connect_block_mixed_ecdsa_schnorr.time_per_block_ms //
                .benchmarks["'"$BENCH_KEY"'"].core.benchmarks[0].time_ms //
                .benchmarks["'"$BENCH_KEY"'"].core.benchmarks[0].time_per_block_ms //
                .benchmarks["'"$BENCH_KEY"'"].core.benchmarks[0].time_ns // 
                (.benchmarks["'"$BENCH_KEY"'"].core.benchmarks[0].time_ns / 1000000) //
                .benchmarks["'"$BENCH_KEY"'"].core.time_ms //
                .benchmarks["'"$BENCH_KEY"'"].core.time_per_block_ms //
                .benchmarks["'"$BENCH_KEY"'"].core.time_ns //
                (.benchmarks["'"$BENCH_KEY"'"].core.time_ns / 1000000) //
                empty
            ' "$OUTPUT_FILE" 2>/dev/null || echo "")
            
            # Comprehensive extraction for Commons - try all possible paths
            COMMONS_TIME=$(jq -r '
                .benchmarks["'"$BENCH_KEY"'"].commons.bitcoin_commons_block_validation.connect_block.time_per_block_ms //
                .benchmarks["'"$BENCH_KEY"'"].commons.benchmarks[0].time_ms //
                .benchmarks["'"$BENCH_KEY"'"].commons.benchmarks[0].time_per_block_ms //
                .benchmarks["'"$BENCH_KEY"'"].commons.benchmarks[0].time_ns //
                (.benchmarks["'"$BENCH_KEY"'"].commons.benchmarks[0].time_ns / 1000000) //
                .benchmarks["'"$BENCH_KEY"'"].commons.time_ms //
                .benchmarks["'"$BENCH_KEY"'"].commons.time_per_block_ms //
                .benchmarks["'"$BENCH_KEY"'"].commons.time_ns //
                (.benchmarks["'"$BENCH_KEY"'"].commons.time_ns / 1000000) //
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
            # Search recursively through the entire structure for time-related fields
            if [ -z "$CORE_TIME" ] || [ "$CORE_TIME" = "null" ] || [ "$CORE_TIME" = "0" ]; then
                # Try to find any field with "time" in the name
                CORE_TIME=$(jq -r '
                    .benchmarks["'"$BENCH_KEY"'"].core | 
                    .. | 
                    select(type == "object") | 
                    to_entries[] | 
                    select(.key | test("time"; "i")) | 
                    select(.value | type == "number" and . > 0) | 
                    .value
                ' "$OUTPUT_FILE" 2>/dev/null | head -1 || echo "")
                
                # If still nothing, try any numeric field > 0
                if [ -z "$CORE_TIME" ] || [ "$CORE_TIME" = "null" ] || [ "$CORE_TIME" = "0" ]; then
                    CORE_TIME=$(jq -r '.benchmarks["'"$BENCH_KEY"'"].core | .. | select(type == "number" and . > 0 and . < 1000000)' "$OUTPUT_FILE" 2>/dev/null | head -1 || echo "")
                fi
            fi
            
            if [ -z "$COMMONS_TIME" ] || [ "$COMMONS_TIME" = "null" ] || [ "$COMMONS_TIME" = "0" ]; then
                # Try to find any field with "time" in the name
                COMMONS_TIME=$(jq -r '
                    .benchmarks["'"$BENCH_KEY"'"].commons | 
                    .. | 
                    select(type == "object") | 
                    to_entries[] | 
                    select(.key | test("time"; "i")) | 
                    select(.value | type == "number" and . > 0) | 
                    .value
                ' "$OUTPUT_FILE" 2>/dev/null | head -1 || echo "")
                
                # If still nothing, try any numeric field > 0
                if [ -z "$COMMONS_TIME" ] || [ "$COMMONS_TIME" = "null" ] || [ "$COMMONS_TIME" = "0" ]; then
                    COMMONS_TIME=$(jq -r '.benchmarks["'"$BENCH_KEY"'"].commons | .. | select(type == "number" and . > 0 and . < 1000000)' "$OUTPUT_FILE" 2>/dev/null | head -1 || echo "")
                fi
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
                # Ensure all values are valid JSON before passing
                # Use temp files for complex JSON objects, --arg for simple values
                TEMP_CORE_STATS=$(mktemp)
                TEMP_COMMONS_STATS=$(mktemp)
                echo "${CORE_STATS:-null}" | jq . > "$TEMP_CORE_STATS" 2>/dev/null || echo "null" > "$TEMP_CORE_STATS"
                echo "${COMMONS_STATS:-null}" | jq . > "$TEMP_COMMONS_STATS" 2>/dev/null || echo "null" > "$TEMP_COMMONS_STATS"
                
                # Ensure numeric values are valid JSON numbers
                # Use awk to validate and convert to numbers, defaulting to safe values
                # Convert to JSON-safe numbers (must be valid JSON, not shell strings)
                SPEEDUP_NUM=$(awk "BEGIN {printf \"%.2f\", ($SPEEDUP + 0)}" 2>/dev/null || echo "1.0")
                CORE_TIME_NUM=$(awk "BEGIN {printf \"%.2f\", ($CORE_TIME + 0)}" 2>/dev/null || echo "0.0")
                COMMONS_TIME_NUM=$(awk "BEGIN {printf \"%.2f\", ($COMMONS_TIME + 0)}" 2>/dev/null || echo "0.0")
                
                # Validate they're actually numbers (must match JSON number format)
                if ! echo "$SPEEDUP_NUM" | grep -qE '^-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?$'; then
                    SPEEDUP_NUM="1.0"
                fi
                if ! echo "$CORE_TIME_NUM" | grep -qE '^-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?$'; then
                    CORE_TIME_NUM="0.0"
                fi
                if ! echo "$COMMONS_TIME_NUM" | grep -qE '^-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?$'; then
                    COMMONS_TIME_NUM="0.0"
                fi
                
                # Convert to valid JSON numbers using jq (must be valid JSON, not shell strings)
                # Use jq to parse the number and output as JSON number
                SPEEDUP_VAL=$(echo "$SPEEDUP_NUM" | jq -c '. // 1.0' 2>/dev/null | jq -r '.' 2>/dev/null || echo "1.0")
                CORE_TIME_VAL=$(echo "$CORE_TIME_NUM" | jq -c '. // 0.0' 2>/dev/null | jq -r '.' 2>/dev/null || echo "0.0")
                COMMONS_TIME_VAL=$(echo "$COMMONS_TIME_NUM" | jq -c '. // 0.0' 2>/dev/null | jq -r '.' 2>/dev/null || echo "0.0")
                
                # Final validation: ensure they're valid numbers (use awk to validate)
                if ! awk "BEGIN {if ($SPEEDUP_VAL != $SPEEDUP_VAL) exit 1}" 2>/dev/null; then
                    SPEEDUP_VAL="1.0"
                fi
                if ! awk "BEGIN {if ($CORE_TIME_VAL != $CORE_TIME_VAL) exit 1}" 2>/dev/null; then
                    CORE_TIME_VAL="0.0"
                fi
                if ! awk "BEGIN {if ($COMMONS_TIME_VAL != $COMMONS_TIME_VAL) exit 1}" 2>/dev/null; then
                    COMMONS_TIME_VAL="0.0"
                fi
                
                # Ensure they're valid JSON numbers
                # Create JSON numbers directly using jq -n (more reliable)
                # First ensure the numbers are valid by using awk to format them
                SPEEDUP_FORMATTED=$(awk "BEGIN {printf \"%.2f\", ($SPEEDUP_NUM + 0)}" 2>/dev/null || echo "1.0")
                CORE_TIME_FORMATTED=$(awk "BEGIN {printf \"%.2f\", ($CORE_TIME_NUM + 0)}" 2>/dev/null || echo "0.0")
                COMMONS_TIME_FORMATTED=$(awk "BEGIN {printf \"%.2f\", ($COMMONS_TIME_NUM + 0)}" 2>/dev/null || echo "0.0")
                
                # Create temp files with JSON numbers using jq -n to ensure valid JSON
                TEMP_SPEEDUP=$(mktemp)
                TEMP_CORE_TIME=$(mktemp)
                TEMP_COMMONS_TIME=$(mktemp)
                
                # Create JSON numbers using printf (more reliable, no jq parsing needed)
                # Validate numbers first
                SPEEDUP_FORMATTED=$(printf "%.2f" "$SPEEDUP_FORMATTED" 2>/dev/null || echo "1.0")
                CORE_TIME_FORMATTED=$(printf "%.2f" "$CORE_TIME_FORMATTED" 2>/dev/null || echo "0.0")
                COMMONS_TIME_FORMATTED=$(printf "%.2f" "$COMMONS_TIME_FORMATTED" 2>/dev/null || echo "0.0")
                
                # Write numbers directly (they're already valid JSON numbers)
                printf "%.2f" "$SPEEDUP_FORMATTED" > "$TEMP_SPEEDUP" 2>/dev/null || echo "1.0" > "$TEMP_SPEEDUP"
                printf "%.2f" "$CORE_TIME_FORMATTED" > "$TEMP_CORE_TIME" 2>/dev/null || echo "0.0" > "$TEMP_CORE_TIME"
                printf "%.2f" "$COMMONS_TIME_FORMATTED" > "$TEMP_COMMONS_TIME" 2>/dev/null || echo "0.0" > "$TEMP_COMMONS_TIME"
                
                # Read numbers from temp files
                SPEEDUP_VAL=$(cat "$TEMP_SPEEDUP" 2>/dev/null || echo "1.0")
                CORE_TIME_VAL=$(cat "$TEMP_CORE_TIME" 2>/dev/null || echo "0.0")
                COMMONS_TIME_VAL=$(cat "$TEMP_COMMONS_TIME" 2>/dev/null || echo "0.0")
                
                # Create comparison JSON using direct number substitution (no --argjson needed)
                # Use jq -n with direct values in the expression
                COMPARISON_JSON=$(jq -n \
                    --arg winner "$WINNER" \
                    --slurpfile core_stats "$TEMP_CORE_STATS" \
                    --slurpfile commons_stats "$TEMP_COMMONS_STATS" \
                    "{
                        winner: \$winner,
                        speedup: $SPEEDUP_VAL,
                        core_time_ms: $CORE_TIME_VAL,
                        commons_time_ms: $COMMONS_TIME_VAL,
                        core_statistics: \$core_stats[0],
                        commons_statistics: \$commons_stats[0]
                    }" 2>/dev/null || jq -n --arg winner "$WINNER" '{
                        winner: $winner,
                        speedup: 1.0,
                        core_time_ms: 0.0,
                        commons_time_ms: 0.0,
                        core_statistics: null,
                        commons_statistics: null
                    }')
                
                rm -f "$TEMP_CORE_STATS" "$TEMP_COMMONS_STATS" "$TEMP_SPEEDUP" "$TEMP_CORE_TIME" "$TEMP_COMMONS_TIME"
                
                # Use temp file for comparison JSON to avoid shell escaping issues
                TEMP_COMP=$(mktemp)
                echo "$COMPARISON_JSON" > "$TEMP_COMP"
                if jq . "$TEMP_COMP" >/dev/null 2>&1; then
                    jq --arg key "$BENCH_KEY" --slurpfile comparison "$TEMP_COMP" \
                       '.benchmarks[$key].comparison = $comparison[0]' \
                       "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
                fi
                rm -f "$TEMP_COMP"
            fi
        fi
    fi
    
    BENCH_COUNT=$((BENCH_COUNT + 1))
done <<< "$JSON_FILES"

# Update summary
# Ensure all counts are valid numbers
TOTAL_COUNT=${BENCH_COUNT:-0}
CORE_COUNT_VAL=${CORE_COUNT:-0}
COMMONS_COUNT_VAL=${COMMONS_COUNT:-0}
COMPARISONS_VAL=${COMPARISON_COUNT:-0}

# Use temp files with JSON numbers for --slurpfile (more reliable than --argjson)
TEMP_TOTAL=$(mktemp)
TEMP_CORE=$(mktemp)
TEMP_COMMONS=$(mktemp)
TEMP_COMPARISONS=$(mktemp)

# Create JSON numbers - use jq -n to create valid JSON numbers directly
# Ensure values are valid integers first
TOTAL_COUNT_INT=$(awk "BEGIN {printf \"%d\", ($TOTAL_COUNT + 0)}" 2>/dev/null || echo "0")
CORE_COUNT_INT=$(awk "BEGIN {printf \"%d\", ($CORE_COUNT_VAL + 0)}" 2>/dev/null || echo "0")
COMMONS_COUNT_INT=$(awk "BEGIN {printf \"%d\", ($COMMONS_COUNT_VAL + 0)}" 2>/dev/null || echo "0")
COMPARISONS_COUNT_INT=$(awk "BEGIN {printf \"%d\", ($COMPARISONS_VAL + 0)}" 2>/dev/null || echo "0")

# Use jq -n to create valid JSON numbers - ensure numbers are valid first
# Validate and sanitize numbers
TOTAL_COUNT_INT=$(printf "%d" "$TOTAL_COUNT_INT" 2>/dev/null || echo "0")
CORE_COUNT_INT=$(printf "%d" "$CORE_COUNT_INT" 2>/dev/null || echo "0")
COMMONS_COUNT_INT=$(printf "%d" "$COMMONS_COUNT_INT" 2>/dev/null || echo "0")
COMPARISONS_COUNT_INT=$(printf "%d" "$COMPARISONS_COUNT_INT" 2>/dev/null || echo "0")

# Create JSON numbers using printf (more reliable than jq -n for simple numbers)
printf "%d" "$TOTAL_COUNT_INT" > "$TEMP_TOTAL" 2>/dev/null || echo "0" > "$TEMP_TOTAL"
printf "%d" "$CORE_COUNT_INT" > "$TEMP_CORE" 2>/dev/null || echo "0" > "$TEMP_CORE"
printf "%d" "$COMMONS_COUNT_INT" > "$TEMP_COMMONS" 2>/dev/null || echo "0" > "$TEMP_COMMONS"
printf "%d" "$COMPARISONS_COUNT_INT" > "$TEMP_COMPARISONS" 2>/dev/null || echo "0" > "$TEMP_COMPARISONS"

# Use --slurpfile to read numbers from temp files
# First, ensure temp files contain valid JSON numbers (just the number, no quotes)
# Numbers written by printf are already valid JSON, but --slurpfile expects JSON arrays
# So we need to wrap them in arrays or use a different approach

# Read numbers and use them directly in jq expression (simpler and more reliable)
jq ".summary.total_benchmarks = $(cat "$TEMP_TOTAL" 2>/dev/null || echo "0") | 
     .summary.core_benchmarks = $(cat "$TEMP_CORE" 2>/dev/null || echo "0") | 
     .summary.commons_benchmarks = $(cat "$TEMP_COMMONS" 2>/dev/null || echo "0") | 
     .summary.comparisons = $(cat "$TEMP_COMPARISONS" 2>/dev/null || echo "0")" \
   "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" 2>/dev/null && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE" || {
    echo "⚠️  Warning: Failed to update summary, using fallback" >&2
    # Fallback: use direct values
    jq ".summary.total_benchmarks = $TOTAL_COUNT_INT | .summary.core_benchmarks = $CORE_COUNT_INT | .summary.commons_benchmarks = $COMMONS_COUNT_INT | .summary.comparisons = $COMPARISONS_COUNT_INT" \
       "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" 2>/dev/null && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE" || true
}

rm -f "$TEMP_TOTAL" "$TEMP_CORE" "$TEMP_COMMONS" "$TEMP_COMPARISONS"

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

