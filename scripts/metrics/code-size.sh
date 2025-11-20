#!/bin/bash
# Codebase Size Metrics Collection
# Collects lines of code, file counts, and module breakdowns for Core and Commons

set +e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/shared/metrics-common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/metrics-code-size-$(date +%Y%m%d-%H%M%S).json"

echo "=== Codebase Size Metrics Collection ==="
echo ""

# Initialize JSON output
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "metric_type": "code_size",
  "bitcoin_core": {},
  "bitcoin_commons": {},
  "comparison": {}
}
EOF

# Collect Core metrics
if [ -n "$CORE_PATH" ] && [ -d "$CORE_PATH" ]; then
    echo "Collecting Bitcoin Core code size metrics..."
    echo "  Core path: $CORE_PATH"
    
    # Get total LOC using tokei
    CORE_STATS=$(get_code_size "$CORE_PATH/src" json 2>/dev/null || echo "{}")
    
    # Extract metrics from tokei JSON or fallback
    if echo "$CORE_STATS" | jq -e '.Rust // .C // .C++ // .total' >/dev/null 2>&1; then
        # tokei JSON format
        CORE_TOTAL_LOC=$(echo "$CORE_STATS" | jq -r '[.Rust.code // 0, .C.code // 0, .C++.code // 0] | add' 2>/dev/null || echo "0")
        CORE_TOTAL_SLOC=$(echo "$CORE_STATS" | jq -r '[.Rust.code // 0, .C.code // 0, .C++.code // 0] | add' 2>/dev/null || echo "0")
        CORE_TOTAL_COMMENTS=$(echo "$CORE_STATS" | jq -r '[.Rust.comments // 0, .C.comments // 0, .C++.comments // 0] | add' 2>/dev/null || echo "0")
        CORE_TOTAL_BLANKS=$(echo "$CORE_STATS" | jq -r '[.Rust.blanks // 0, .C.blanks // 0, .C++.blanks // 0] | add' 2>/dev/null || echo "0")
        CORE_TOTAL_FILES=$(echo "$CORE_STATS" | jq -r '[.Rust.blanks // 0, .C.blanks // 0, .C++.blanks // 0] | add' 2>/dev/null || echo "0")
        # Try to get file count from tokei
        CORE_CPP_FILES=$(find "$CORE_PATH/src" -name "*.cpp" -o -name "*.h" 2>/dev/null | wc -l)
        CORE_TOTAL_FILES=$CORE_CPP_FILES
    else
        # Fallback format
        CORE_TOTAL_LOC=$(echo "$CORE_STATS" | jq -r '.total // 0' 2>/dev/null || echo "0")
        CORE_TOTAL_SLOC=$CORE_TOTAL_LOC  # Fallback doesn't separate
        CORE_TOTAL_COMMENTS="0"
        CORE_TOTAL_BLANKS="0"
        CORE_CPP_FILES=$(echo "$CORE_STATS" | jq -r '.files.cpp // 0' 2>/dev/null || echo "0")
        CORE_TOTAL_FILES=$CORE_CPP_FILES
    fi
    
    # Get module breakdown
    CORE_MODULES=$(get_module_breakdown "$CORE_PATH" "core")
    
    # Update JSON
    TEMP_CORE=$(mktemp)
    jq -n \
        --argjson total_loc "$CORE_TOTAL_LOC" \
        --argjson sloc "$CORE_TOTAL_SLOC" \
        --argjson comments "$CORE_TOTAL_COMMENTS" \
        --argjson blanks "$CORE_TOTAL_BLANKS" \
        --argjson files "$CORE_TOTAL_FILES" \
        --argjson cpp_files "$CORE_CPP_FILES" \
        --argjson modules "$CORE_MODULES" \
        '{
            total_loc: $total_loc,
            sloc: $sloc,
            comments: $comments,
            blanks: $blanks,
            total_files: $files,
            cpp_files: $cpp_files,
            header_files: 0,
            by_module: $modules
        }' > "$TEMP_CORE" 2>/dev/null || echo '{}' > "$TEMP_CORE"
    
    jq --slurpfile core_data "$TEMP_CORE" '.bitcoin_core = $core_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_CORE"
    
    echo "  ✅ Core metrics collected: $CORE_TOTAL_LOC LOC, $CORE_TOTAL_FILES files"
else
    echo "⚠️  Bitcoin Core path not found, skipping Core metrics"
fi

# Collect Commons metrics (bllvm-consensus and bllvm-node)
COMMONS_TOTAL_LOC=0
COMMONS_TOTAL_SLOC=0
COMMONS_TOTAL_FILES=0
COMMONS_CRATES_JSON="{}"

if [ -n "$COMMONS_CONSENSUS_PATH" ] && [ -d "$COMMONS_CONSENSUS_PATH" ]; then
    echo "Collecting Bitcoin Commons code size metrics..."
    
    # bllvm-consensus
    if [ -d "$COMMONS_CONSENSUS_PATH/src" ]; then
        echo "  Analyzing bllvm-consensus..."
        CONSENSUS_STATS=$(get_code_size "$COMMONS_CONSENSUS_PATH/src" json 2>/dev/null || echo "{}")
        CONSENSUS_LOC=$(echo "$CONSENSUS_STATS" | jq -r '.Rust.code // .total // 0' 2>/dev/null || echo "0")
        CONSENSUS_SLOC=$CONSENSUS_LOC
        CONSENSUS_FILES=$(find "$COMMONS_CONSENSUS_PATH/src" -name "*.rs" 2>/dev/null | wc -l)
        
        COMMONS_TOTAL_LOC=$((COMMONS_TOTAL_LOC + CONSENSUS_LOC))
        COMMONS_TOTAL_SLOC=$((COMMONS_TOTAL_SLOC + CONSENSUS_SLOC))
        COMMONS_TOTAL_FILES=$((COMMONS_TOTAL_FILES + CONSENSUS_FILES))
        
        CONSENSUS_BREAKDOWN=$(get_module_breakdown "$COMMONS_CONSENSUS_PATH" "commons")
        COMMONS_CRATES_JSON=$(echo "$COMMONS_CRATES_JSON" | jq --argjson data "$CONSENSUS_BREAKDOWN" \
            '. + {"bllvm-consensus": $data}' 2>/dev/null || echo "$COMMONS_CRATES_JSON")
        
        echo "    bllvm-consensus: $CONSENSUS_LOC LOC, $CONSENSUS_FILES files"
    fi
fi

if [ -n "$COMMONS_NODE_PATH" ] && [ -d "$COMMONS_NODE_PATH" ]; then
    # bllvm-node
    if [ -d "$COMMONS_NODE_PATH/src" ]; then
        echo "  Analyzing bllvm-node..."
        NODE_STATS=$(get_code_size "$COMMONS_NODE_PATH/src" json 2>/dev/null || echo "{}")
        NODE_LOC=$(echo "$NODE_STATS" | jq -r '.Rust.code // .total // 0' 2>/dev/null || echo "0")
        NODE_SLOC=$NODE_LOC
        NODE_FILES=$(find "$COMMONS_NODE_PATH/src" -name "*.rs" 2>/dev/null | wc -l)
        
        COMMONS_TOTAL_LOC=$((COMMONS_TOTAL_LOC + NODE_LOC))
        COMMONS_TOTAL_SLOC=$((COMMONS_TOTAL_SLOC + NODE_SLOC))
        COMMONS_TOTAL_FILES=$((COMMONS_TOTAL_FILES + NODE_FILES))
        
        NODE_BREAKDOWN=$(get_module_breakdown "$COMMONS_NODE_PATH" "commons")
        COMMONS_CRATES_JSON=$(echo "$COMMONS_CRATES_JSON" | jq --argjson data "$NODE_BREAKDOWN" \
            '. + {"bllvm-node": $data}' 2>/dev/null || echo "$COMMONS_CRATES_JSON")
        
        echo "    bllvm-node: $NODE_LOC LOC, $NODE_FILES files"
    fi
fi

if [ "$COMMONS_TOTAL_LOC" -gt 0 ]; then
    # Update JSON with Commons data
    TEMP_COMMONS=$(mktemp)
    jq -n \
        --argjson total_loc "$COMMONS_TOTAL_LOC" \
        --argjson sloc "$COMMONS_TOTAL_SLOC" \
        --argjson files "$COMMONS_TOTAL_FILES" \
        --argjson crates "$COMMONS_CRATES_JSON" \
        '{
            total_loc: $total_loc,
            sloc: $sloc,
            total_files: $files,
            rust_files: $files,
            by_crate: $crates
        }' > "$TEMP_COMMONS" 2>/dev/null || echo '{}' > "$TEMP_COMMONS"
    
    jq --slurpfile commons_data "$TEMP_COMMONS" '.bitcoin_commons = $commons_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_COMMONS"
    
    echo "  ✅ Commons metrics collected: $COMMONS_TOTAL_LOC LOC, $COMMONS_TOTAL_FILES files"
else
    echo "⚠️  Bitcoin Commons paths not found, skipping Commons metrics"
fi

# Calculate comparison metrics
CORE_LOC_VAL=$(jq -r '.bitcoin_core.total_loc // 0' "$OUTPUT_FILE" 2>/dev/null || echo "0")
COMMONS_LOC_VAL=$(jq -r '.bitcoin_commons.total_loc // 0' "$OUTPUT_FILE" 2>/dev/null || echo "0")
CORE_FILES_VAL=$(jq -r '.bitcoin_core.total_files // 0' "$OUTPUT_FILE" 2>/dev/null || echo "0")
COMMONS_FILES_VAL=$(jq -r '.bitcoin_commons.total_files // 0' "$OUTPUT_FILE" 2>/dev/null || echo "0")

if [ "$CORE_LOC_VAL" != "0" ] && [ "$COMMONS_LOC_VAL" != "0" ]; then
    LOC_RATIO=$(awk "BEGIN {printf \"%.2f\", $COMMONS_LOC_VAL / $CORE_LOC_VAL}" 2>/dev/null || echo "0")
    FILES_RATIO=$(awk "BEGIN {printf \"%.2f\", $COMMONS_FILES_VAL / $CORE_FILES_VAL}" 2>/dev/null || echo "0")
    
    TEMP_COMP=$(mktemp)
    jq -n \
        --argjson loc_ratio "$LOC_RATIO" \
        --argjson files_ratio "$FILES_RATIO" \
        --argjson core_loc "$CORE_LOC_VAL" \
        --argjson commons_loc "$COMMONS_LOC_VAL" \
        '{
            loc_ratio: $loc_ratio,
            files_ratio: $files_ratio,
            core_total_loc: $core_loc,
            commons_total_loc: $commons_loc,
            analysis: "Rust is more expressive and typically requires fewer lines for equivalent functionality. Direct LOC comparison is not meaningful due to language differences."
        }' > "$TEMP_COMP" 2>/dev/null || echo '{}' > "$TEMP_COMP"
    
    jq --slurpfile comp_data "$TEMP_COMP" '.comparison = $comp_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_COMP"
fi

echo ""
echo "✅ Results saved to: $OUTPUT_FILE"
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

exit 0

