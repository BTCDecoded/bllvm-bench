#!/bin/bash
# Basic Test Metrics Collection
# Collects test file counts and LOC for Core and Commons

set +e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/shared/metrics-common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/metrics-tests-$(date +%Y%m%d-%H%M%S).json"

echo "=== Basic Test Metrics Collection ==="
echo ""

# Initialize JSON output
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "metric_type": "tests",
  "bitcoin_core": {},
  "bitcoin_commons": {},
  "comparison": {}
}
EOF

# Collect Core test metrics
if [ -n "$CORE_PATH" ] && [ -d "$CORE_PATH" ]; then
    echo "Collecting Bitcoin Core test metrics..."
    echo "  Core path: $CORE_PATH"
    
    # Find test files
    CORE_TEST_FILES=$(find "$CORE_PATH" -type f \( -name "*test*.cpp" -o -name "*test*.h" -o -name "*_tests.cpp" \) \
        ! -path "*/build/*" ! -path "*/depends/*" 2>/dev/null | wc -l)
    
    # Count test LOC
    CORE_TEST_LOC=$(find "$CORE_PATH" -type f \( -name "*test*.cpp" -o -name "*_tests.cpp" \) \
        ! -path "*/build/*" ! -path "*/depends/*" 2>/dev/null | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1}' || echo "0")
    
    # Get production LOC for ratio
    CORE_PROD_LOC=$(jq -r '.bitcoin_core.total_loc // 0' "$OUTPUT_DIR/metrics-code-size-"*.json 2>/dev/null | head -1 || echo "0")
    if [ "$CORE_PROD_LOC" = "0" ]; then
        # Fallback: count production LOC directly
        CORE_PROD_LOC=$(find "$CORE_PATH/src" -type f \( -name "*.cpp" -o -name "*.h" \) \
            ! -name "*test*" 2>/dev/null | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1}' || echo "0")
    fi
    
    # Calculate ratio
    if [ "$CORE_PROD_LOC" != "0" ]; then
        CORE_TEST_RATIO=$(awk "BEGIN {printf \"%.3f\", $CORE_TEST_LOC / $CORE_PROD_LOC}" 2>/dev/null || echo "0")
    else
        CORE_TEST_RATIO="0"
    fi
    
    # Update JSON
    TEMP_CORE=$(mktemp)
    jq -n \
        --argjson test_files "$CORE_TEST_FILES" \
        --argjson test_loc "$CORE_TEST_LOC" \
        --argjson prod_loc "$CORE_PROD_LOC" \
        --argjson ratio "$CORE_TEST_RATIO" \
        '{
            test_files: $test_files,
            test_loc: $test_loc,
            production_loc: $prod_loc,
            test_to_production_ratio: $ratio,
            note: "Test files identified by *test*.cpp and *_tests.cpp patterns"
        }' > "$TEMP_CORE" 2>/dev/null || echo '{}' > "$TEMP_CORE"
    
    jq --slurpfile core_data "$TEMP_CORE" '.bitcoin_core = $core_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_CORE"
    
    echo "  ✅ Core tests: $CORE_TEST_FILES files, $CORE_TEST_LOC LOC (ratio: $CORE_TEST_RATIO)"
else
    echo "⚠️  Bitcoin Core path not found, skipping Core test metrics"
fi

# Collect Commons test metrics
COMMONS_TOTAL_TEST_FILES=0
COMMONS_TOTAL_TEST_LOC=0
COMMONS_TOTAL_PROD_LOC=0
COMMONS_CRATES_TESTS="{}"

if [ -n "$COMMONS_CONSENSUS_PATH" ] && [ -d "$COMMONS_CONSENSUS_PATH" ]; then
    echo "Collecting Bitcoin Commons test metrics..."
    
    # bllvm-consensus
    if [ -d "$COMMONS_CONSENSUS_PATH" ]; then
        echo "  Analyzing bllvm-consensus tests..."
        
        # Test files in tests/ directory
        CONSENSUS_TEST_FILES=$(find "$COMMONS_CONSENSUS_PATH/tests" -name "*.rs" 2>/dev/null | wc -l)
        # Test files in src/ (integration tests with #[cfg(test)])
        CONSENSUS_TEST_FILES=$((CONSENSUS_TEST_FILES + $(find "$COMMONS_CONSENSUS_PATH/src" -name "*.rs" -exec grep -l '#\[cfg(test)\]' {} \; 2>/dev/null | wc -l)))
        
        # Test LOC
        CONSENSUS_TEST_LOC=$(find "$COMMONS_CONSENSUS_PATH/tests" -name "*.rs" 2>/dev/null | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1}' || echo "0")
        
        # Production LOC
        CONSENSUS_PROD_LOC=$(jq -r '.bitcoin_commons.by_crate."bllvm-consensus".loc // 0' "$OUTPUT_DIR/metrics-code-size-"*.json 2>/dev/null | head -1 || echo "0")
        if [ "$CONSENSUS_PROD_LOC" = "0" ]; then
            CONSENSUS_PROD_LOC=$(find "$COMMONS_CONSENSUS_PATH/src" -name "*.rs" \
                ! -exec grep -l '#\[cfg(test)\]' {} \; 2>/dev/null | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1}' || echo "0")
        fi
        
        COMMONS_TOTAL_TEST_FILES=$((COMMONS_TOTAL_TEST_FILES + CONSENSUS_TEST_FILES))
        COMMONS_TOTAL_TEST_LOC=$((COMMONS_TOTAL_TEST_LOC + CONSENSUS_TEST_LOC))
        COMMONS_TOTAL_PROD_LOC=$((COMMONS_TOTAL_PROD_LOC + CONSENSUS_PROD_LOC))
        
        if [ "$CONSENSUS_PROD_LOC" != "0" ]; then
            CONSENSUS_RATIO=$(awk "BEGIN {printf \"%.3f\", $CONSENSUS_TEST_LOC / $CONSENSUS_PROD_LOC}" 2>/dev/null || echo "0")
        else
            CONSENSUS_RATIO="0"
        fi
        
        CONSENSUS_TESTS_JSON=$(jq -n \
            --argjson files "$CONSENSUS_TEST_FILES" \
            --argjson loc "$CONSENSUS_TEST_LOC" \
            --argjson ratio "$CONSENSUS_RATIO" \
            '{"test_files": $files, "test_loc": $loc, "test_to_production_ratio": $ratio}' 2>/dev/null || echo "{}")
        
        COMMONS_CRATES_TESTS=$(echo "$COMMONS_CRATES_TESTS" | jq --argjson data "$CONSENSUS_TESTS_JSON" \
            '. + {"bllvm-consensus": $data}' 2>/dev/null || echo "$COMMONS_CRATES_TESTS")
        
        echo "    bllvm-consensus: $CONSENSUS_TEST_FILES test files, $CONSENSUS_TEST_LOC LOC (ratio: $CONSENSUS_RATIO)"
    fi
fi

if [ -n "$COMMONS_NODE_PATH" ] && [ -d "$COMMONS_NODE_PATH" ]; then
    # bllvm-node
    if [ -d "$COMMONS_NODE_PATH" ]; then
        echo "  Analyzing bllvm-node tests..."
        
        NODE_TEST_FILES=$(find "$COMMONS_NODE_PATH/tests" -name "*.rs" 2>/dev/null | wc -l)
        NODE_TEST_FILES=$((NODE_TEST_FILES + $(find "$COMMONS_NODE_PATH/src" -name "*.rs" -exec grep -l '#\[cfg(test)\]' {} \; 2>/dev/null | wc -l)))
        
        NODE_TEST_LOC=$(find "$COMMONS_NODE_PATH/tests" -name "*.rs" 2>/dev/null | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1}' || echo "0")
        
        NODE_PROD_LOC=$(jq -r '.bitcoin_commons.by_crate."bllvm-node".loc // 0' "$OUTPUT_DIR/metrics-code-size-"*.json 2>/dev/null | head -1 || echo "0")
        if [ "$NODE_PROD_LOC" = "0" ]; then
            NODE_PROD_LOC=$(find "$COMMONS_NODE_PATH/src" -name "*.rs" \
                ! -exec grep -l '#\[cfg(test)\]' {} \; 2>/dev/null | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1}' || echo "0")
        fi
        
        COMMONS_TOTAL_TEST_FILES=$((COMMONS_TOTAL_TEST_FILES + NODE_TEST_FILES))
        COMMONS_TOTAL_TEST_LOC=$((COMMONS_TOTAL_TEST_LOC + NODE_TEST_LOC))
        COMMONS_TOTAL_PROD_LOC=$((COMMONS_TOTAL_PROD_LOC + NODE_PROD_LOC))
        
        if [ "$NODE_PROD_LOC" != "0" ]; then
            NODE_RATIO=$(awk "BEGIN {printf \"%.3f\", $NODE_TEST_LOC / $NODE_PROD_LOC}" 2>/dev/null || echo "0")
        else
            NODE_RATIO="0"
        fi
        
        NODE_TESTS_JSON=$(jq -n \
            --argjson files "$NODE_TEST_FILES" \
            --argjson loc "$NODE_TEST_LOC" \
            --argjson ratio "$NODE_RATIO" \
            '{"test_files": $files, "test_loc": $loc, "test_to_production_ratio": $ratio}' 2>/dev/null || echo "{}")
        
        COMMONS_CRATES_TESTS=$(echo "$COMMONS_CRATES_TESTS" | jq --argjson data "$NODE_TESTS_JSON" \
            '. + {"bllvm-node": $data}' 2>/dev/null || echo "$COMMONS_CRATES_TESTS")
        
        echo "    bllvm-node: $NODE_TEST_FILES test files, $NODE_TEST_LOC LOC (ratio: $NODE_RATIO)"
    fi
fi

if [ "$COMMONS_TOTAL_TEST_FILES" -gt 0 ]; then
    # Calculate overall ratio
    if [ "$COMMONS_TOTAL_PROD_LOC" != "0" ]; then
        COMMONS_TOTAL_RATIO=$(awk "BEGIN {printf \"%.3f\", $COMMONS_TOTAL_TEST_LOC / $COMMONS_TOTAL_PROD_LOC}" 2>/dev/null || echo "0")
    else
        COMMONS_TOTAL_RATIO="0"
    fi
    
    # Update JSON with Commons data
    TEMP_COMMONS=$(mktemp)
    jq -n \
        --argjson test_files "$COMMONS_TOTAL_TEST_FILES" \
        --argjson test_loc "$COMMONS_TOTAL_TEST_LOC" \
        --argjson prod_loc "$COMMONS_TOTAL_PROD_LOC" \
        --argjson ratio "$COMMONS_TOTAL_RATIO" \
        --argjson crates "$COMMONS_CRATES_TESTS" \
        '{
            test_files: $test_files,
            test_loc: $test_loc,
            production_loc: $prod_loc,
            test_to_production_ratio: $ratio,
            by_crate: $crates,
            note: "Test files include tests/ directory and #[cfg(test)] modules in src/"
        }' > "$TEMP_COMMONS" 2>/dev/null || echo '{}' > "$TEMP_COMMONS"
    
    jq --slurpfile commons_data "$TEMP_COMMONS" '.bitcoin_commons = $commons_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_COMMONS"
    
    echo "  ✅ Commons tests: $COMMONS_TOTAL_TEST_FILES files, $COMMONS_TOTAL_TEST_LOC LOC (ratio: $COMMONS_TOTAL_RATIO)"
else
    echo "⚠️  Bitcoin Commons paths not found, skipping Commons test metrics"
fi

# Calculate comparison
CORE_TEST_FILES_VAL=$(jq -r '.bitcoin_core.test_files // 0' "$OUTPUT_FILE" 2>/dev/null || echo "0")
COMMONS_TEST_FILES_VAL=$(jq -r '.bitcoin_commons.test_files // 0' "$OUTPUT_FILE" 2>/dev/null || echo "0")
CORE_RATIO_VAL=$(jq -r '.bitcoin_core.test_to_production_ratio // 0' "$OUTPUT_FILE" 2>/dev/null || echo "0")
COMMONS_RATIO_VAL=$(jq -r '.bitcoin_commons.test_to_production_ratio // 0' "$OUTPUT_FILE" 2>/dev/null || echo "0")

if [ "$CORE_TEST_FILES_VAL" != "0" ] && [ "$COMMONS_TEST_FILES_VAL" != "0" ]; then
    RATIO_DELTA=$(awk "BEGIN {printf \"%.3f\", $COMMONS_RATIO_VAL - $CORE_RATIO_VAL}" 2>/dev/null || echo "0")
    
    TEMP_COMP=$(mktemp)
    jq -n \
        --argjson core_files "$CORE_TEST_FILES_VAL" \
        --argjson commons_files "$COMMONS_TEST_FILES_VAL" \
        --argjson core_ratio "$CORE_RATIO_VAL" \
        --argjson commons_ratio "$COMMONS_RATIO_VAL" \
        --argjson delta "$RATIO_DELTA" \
        '{
            test_files_delta: ($commons_files - $core_files),
            ratio_delta: $delta,
            core_test_ratio: $core_ratio,
            commons_test_ratio: $commons_ratio,
            analysis: "Test-to-production ratio indicates test coverage intensity. Higher ratio suggests more comprehensive testing."
        }' > "$TEMP_COMP" 2>/dev/null || echo '{}' > "$TEMP_COMP"
    
    jq --slurpfile comp_data "$TEMP_COMP" '.comparison = $comp_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_COMP"
fi

echo ""
echo "✅ Results saved to: $OUTPUT_FILE"
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

exit 0

