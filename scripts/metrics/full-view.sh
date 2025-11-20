#!/bin/bash
# Full View Metrics
# Combines code + feature flags + tests + comments into complete codebase view

set +e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/shared/metrics-common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/metrics-full-view-$(date +%Y%m%d-%H%M%S).json"

echo "=== Full View Metrics (Code + Features + Tests + Comments) ==="
echo ""

# Find latest metrics files
CODE_SIZE_FILE=$(ls -t "$OUTPUT_DIR/metrics-code-size-"*.json 2>/dev/null | head -1)
COMBINED_FILE=$(ls -t "$OUTPUT_DIR/metrics-combined-view-"*.json 2>/dev/null | head -1)

# Initialize JSON output
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "metric_type": "full_view",
  "bitcoin_core": {},
  "bitcoin_commons": {},
  "comparison": {}
}
EOF

# Use combined view as base, add comment analysis
if [ -f "$CODE_SIZE_FILE" ]; then
    echo "Building full view from:"
    echo "  Code size: $(basename "$CODE_SIZE_FILE")"
    [ -f "$COMBINED_FILE" ] && echo "  Combined view: $(basename "$COMBINED_FILE")"
    echo ""
    
    # Extract comment data from code size metrics (tokei provides this)
    CORE_CODE=$(jq '.bitcoin_core' "$CODE_SIZE_FILE" 2>/dev/null || echo "{}")
    COMMONS_CODE=$(jq '.bitcoin_commons' "$CODE_SIZE_FILE" 2>/dev/null || echo "{}")
    
    # Get comment counts (from tokei output if available)
    CORE_COMMENTS=$(echo "$CORE_CODE" | jq -r '.comments // 0' 2>/dev/null || echo "0")
    CORE_BLANKS=$(echo "$CORE_CODE" | jq -r '.blanks // 0' 2>/dev/null || echo "0")
    CORE_SLOC=$(echo "$CORE_CODE" | jq -r '.sloc // .total_loc // 0' 2>/dev/null || echo "0")
    
    # Calculate comment density
    if [ "$CORE_SLOC" != "0" ]; then
        CORE_COMMENT_DENSITY=$(awk "BEGIN {printf \"%.3f\", $CORE_COMMENTS / $CORE_SLOC}" 2>/dev/null || echo "0")
    else
        CORE_COMMENT_DENSITY="0"
    fi
    
    # Count documentation files
    if [ -n "$CORE_PATH" ] && [ -d "$CORE_PATH" ]; then
        CORE_DOC_FILES=$(find "$CORE_PATH" -type f \( -name "*.md" -o -name "*.txt" -o -name "*.rst" \) \
            ! -path "*/build/*" ! -path "*/depends/*" ! -path "*/.git/*" 2>/dev/null | wc -l)
    else
        CORE_DOC_FILES="0"
    fi
    
    # Build Core full view
    CORE_TOTAL_LOC=$(echo "$CORE_CODE" | jq -r '.total_loc // 0' 2>/dev/null || echo "0")
    CORE_TEST_LOC=$(jq -r '.bitcoin_core.test_loc // 0' "$OUTPUT_DIR/metrics-tests-"*.json 2>/dev/null | head -1 || echo "0")
    CORE_TOTAL_WITH_COMMENTS=$((CORE_TOTAL_LOC + CORE_COMMENTS))
    
    TEMP_CORE=$(mktemp)
    jq -n \
        --argjson code "$CORE_CODE" \
        --argjson comments "$CORE_COMMENTS" \
        --argjson blanks "$CORE_BLANKS" \
        --argjson sloc "$CORE_SLOC" \
        --argjson density "$CORE_COMMENT_DENSITY" \
        --argjson doc_files "$CORE_DOC_FILES" \
        --argjson total_loc "$CORE_TOTAL_LOC" \
        --argjson test_loc "$CORE_TEST_LOC" \
        --argjson total_with_comments "$CORE_TOTAL_WITH_COMMENTS" \
        '{
            code: $code,
            comments: {
                total_comments: $comments,
                total_blanks: $blanks,
                source_loc: $sloc,
                comment_density: $density,
                documentation_files: $doc_files
            },
            totals: {
                production_loc: $total_loc,
                test_loc: $test_loc,
                comments_loc: $comments,
                blanks_loc: $blanks,
                total_with_comments: $total_with_comments,
                source_lines_only: $sloc
            },
            breakdown: {
                production: $code,
                comments: $comments,
                blanks: $blanks
            }
        }' > "$TEMP_CORE" 2>/dev/null || echo '{}' > "$TEMP_CORE"
    
    jq --slurpfile core_data "$TEMP_CORE" '.bitcoin_core = $core_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_CORE"
    
    echo "✅ Core full view: $CORE_TOTAL_WITH_COMMENTS LOC (code: $CORE_TOTAL_LOC, comments: $CORE_COMMENTS, density: $CORE_COMMENT_DENSITY)"
    
    # Commons full view
    COMMONS_COMMENTS=$(echo "$COMMONS_CODE" | jq -r '.comments // 0' 2>/dev/null || echo "0")
    COMMONS_BLANKS=$(echo "$COMMONS_CODE" | jq -r '.blanks // 0' 2>/dev/null || echo "0")
    COMMONS_SLOC=$(echo "$COMMONS_CODE" | jq -r '.sloc // .total_loc // 0' 2>/dev/null || echo "0")
    
    if [ "$COMMONS_SLOC" != "0" ]; then
        COMMONS_COMMENT_DENSITY=$(awk "BEGIN {printf \"%.3f\", $COMMONS_COMMENTS / $COMMONS_SLOC}" 2>/dev/null || echo "0")
    else
        COMMONS_COMMENT_DENSITY="0"
    fi
    
    # Count documentation files for Commons
    COMMONS_DOC_FILES=0
    if [ -n "$COMMONS_CONSENSUS_PATH" ] && [ -d "$COMMONS_CONSENSUS_PATH" ]; then
        COMMONS_DOC_FILES=$((COMMONS_DOC_FILES + $(find "$COMMONS_CONSENSUS_PATH" -type f \( -name "*.md" -o -name "*.txt" -o -name "*.rst" \) \
            ! -path "*/.git/*" ! -path "*/target/*" 2>/dev/null | wc -l)))
    fi
    if [ -n "$COMMONS_NODE_PATH" ] && [ -d "$COMMONS_NODE_PATH" ]; then
        COMMONS_DOC_FILES=$((COMMONS_DOC_FILES + $(find "$COMMONS_NODE_PATH" -type f \( -name "*.md" -o -name "*.txt" -o -name "*.rst" \) \
            ! -path "*/.git/*" ! -path "*/target/*" 2>/dev/null | wc -l)))
    fi
    
    COMMONS_TOTAL_LOC=$(echo "$COMMONS_CODE" | jq -r '.total_loc // 0' 2>/dev/null || echo "0")
    COMMONS_TEST_LOC=$(jq -r '.bitcoin_commons.test_loc // 0' "$OUTPUT_DIR/metrics-tests-"*.json 2>/dev/null | head -1 || echo "0")
    COMMONS_TOTAL_WITH_COMMENTS=$((COMMONS_TOTAL_LOC + COMMONS_COMMENTS))
    
    TEMP_COMMONS=$(mktemp)
    jq -n \
        --argjson code "$COMMONS_CODE" \
        --argjson comments "$COMMONS_COMMENTS" \
        --argjson blanks "$COMMONS_BLANKS" \
        --argjson sloc "$COMMONS_SLOC" \
        --argjson density "$COMMONS_COMMENT_DENSITY" \
        --argjson doc_files "$COMMONS_DOC_FILES" \
        --argjson total_loc "$COMMONS_TOTAL_LOC" \
        --argjson test_loc "$COMMONS_TEST_LOC" \
        --argjson total_with_comments "$COMMONS_TOTAL_WITH_COMMENTS" \
        '{
            code: $code,
            comments: {
                total_comments: $comments,
                total_blanks: $blanks,
                source_loc: $sloc,
                comment_density: $density,
                documentation_files: $doc_files
            },
            totals: {
                production_loc: $total_loc,
                test_loc: $test_loc,
                comments_loc: $comments,
                blanks_loc: $blanks,
                total_with_comments: $total_with_comments,
                source_lines_only: $sloc
            },
            breakdown: {
                production: $code,
                comments: $comments,
                blanks: $blanks
            }
        }' > "$TEMP_COMMONS" 2>/dev/null || echo '{}' > "$TEMP_COMMONS"
    
    jq --slurpfile commons_data "$TEMP_COMMONS" '.bitcoin_commons = $commons_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_COMMONS"
    
    echo "✅ Commons full view: $COMMONS_TOTAL_WITH_COMMENTS LOC (code: $COMMONS_TOTAL_LOC, comments: $COMMONS_COMMENTS, density: $COMMONS_COMMENT_DENSITY)"
    
    # Calculate comparison
    if [ "$CORE_TOTAL_WITH_COMMENTS" != "0" ] && [ "$COMMONS_TOTAL_WITH_COMMENTS" != "0" ]; then
        TOTAL_RATIO=$(awk "BEGIN {printf \"%.2f\", $COMMONS_TOTAL_WITH_COMMENTS / $CORE_TOTAL_WITH_COMMENTS}" 2>/dev/null || echo "0")
        DENSITY_DELTA=$(awk "BEGIN {printf \"%.3f\", $COMMONS_COMMENT_DENSITY - $CORE_COMMENT_DENSITY}" 2>/dev/null || echo "0")
        
        TEMP_COMP=$(mktemp)
        jq -n \
            --argjson total_ratio "$TOTAL_RATIO" \
            --argjson density_delta "$DENSITY_DELTA" \
            --argjson core_density "$CORE_COMMENT_DENSITY" \
            --argjson commons_density "$COMMONS_COMMENT_DENSITY" \
            --argjson core_total "$CORE_TOTAL_WITH_COMMENTS" \
            --argjson commons_total "$COMMONS_TOTAL_WITH_COMMENTS" \
            '{
                total_loc_ratio: $total_ratio,
                comment_density_delta: $density_delta,
                core_comment_density: $core_density,
                commons_comment_density: $commons_density,
                core_total_with_comments: $core_total,
                commons_total_with_comments: $commons_total,
                analysis: "Full view includes all code, tests, comments, and documentation. Comment density indicates documentation intensity."
            }' > "$TEMP_COMP" 2>/dev/null || echo '{}' > "$TEMP_COMP"
        
        jq --slurpfile comp_data "$TEMP_COMP" '.comparison = $comp_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
        rm -f "$TEMP_COMP"
    fi
else
    echo "⚠️  Code size metrics file not found"
    echo "   Run code-size.sh first"
fi

echo ""
echo "✅ Results saved to: $OUTPUT_FILE"
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

exit 0

