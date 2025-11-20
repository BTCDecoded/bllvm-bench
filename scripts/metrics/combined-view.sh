#!/bin/bash
# Combined View Metrics
# Combines code + feature flags + tests into a unified view

set +e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/shared/metrics-common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/metrics-combined-view-$(date +%Y%m%d-%H%M%S).json"

echo "=== Combined View Metrics (Code + Features + Tests) ==="
echo ""

# Find latest metrics files
CODE_SIZE_FILE=$(ls -t "$OUTPUT_DIR/metrics-code-size-"*.json 2>/dev/null | head -1)
FEATURES_FILE=$(ls -t "$OUTPUT_DIR/metrics-features-"*.json 2>/dev/null | head -1)
TESTS_FILE=$(ls -t "$OUTPUT_DIR/metrics-tests-"*.json 2>/dev/null | head -1)

# Initialize JSON output
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "metric_type": "combined_view",
  "bitcoin_core": {},
  "bitcoin_commons": {},
  "comparison": {}
}
EOF

# Combine Core metrics
if [ -f "$CODE_SIZE_FILE" ] && [ -f "$FEATURES_FILE" ] && [ -f "$TESTS_FILE" ]; then
    echo "Combining metrics from:"
    echo "  Code size: $(basename "$CODE_SIZE_FILE")"
    echo "  Features: $(basename "$FEATURES_FILE")"
    echo "  Tests: $(basename "$TESTS_FILE")"
    echo ""
    
    # Extract Core data
    CORE_CODE=$(jq '.bitcoin_core' "$CODE_SIZE_FILE" 2>/dev/null || echo "{}")
    CORE_FEATURES=$(jq '.bitcoin_core' "$FEATURES_FILE" 2>/dev/null || echo "{}")
    CORE_TESTS=$(jq '.bitcoin_core' "$TESTS_FILE" 2>/dev/null || echo "{}")
    
    # Calculate totals
    CORE_PROD_LOC=$(echo "$CORE_CODE" | jq -r '.total_loc // 0' 2>/dev/null || echo "0")
    CORE_TEST_LOC=$(echo "$CORE_TESTS" | jq -r '.test_loc // 0' 2>/dev/null || echo "0")
    CORE_TOTAL_LOC=$((CORE_PROD_LOC + CORE_TEST_LOC))
    
    # Build combined Core JSON
    TEMP_CORE=$(mktemp)
    jq -n \
        --argjson code "$CORE_CODE" \
        --argjson features "$CORE_FEATURES" \
        --argjson tests "$CORE_TESTS" \
        --argjson prod_loc "$CORE_PROD_LOC" \
        --argjson test_loc "$CORE_TEST_LOC" \
        --argjson total_loc "$CORE_TOTAL_LOC" \
        '{
            production_code: $code,
            features: $features,
            tests: $tests,
            totals: {
                production_loc: $prod_loc,
                test_loc: $test_loc,
                total_loc: $total_loc,
                total_files: ($code.total_files // 0) + ($tests.test_files // 0)
            },
            breakdown: {
                production: $code,
                tests: $tests,
                features: $features
            }
        }' > "$TEMP_CORE" 2>/dev/null || echo '{}' > "$TEMP_CORE"
    
    jq --slurpfile core_data "$TEMP_CORE" '.bitcoin_core = $core_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_CORE"
    
    echo "✅ Core combined view: $CORE_TOTAL_LOC total LOC (prod: $CORE_PROD_LOC, test: $CORE_TEST_LOC)"
    
    # Extract Commons data
    COMMONS_CODE=$(jq '.bitcoin_commons' "$CODE_SIZE_FILE" 2>/dev/null || echo "{}")
    COMMONS_FEATURES=$(jq '.bitcoin_commons' "$FEATURES_FILE" 2>/dev/null || echo "{}")
    COMMONS_TESTS=$(jq '.bitcoin_commons' "$TESTS_FILE" 2>/dev/null || echo "{}")
    
    # Calculate totals
    COMMONS_PROD_LOC=$(echo "$COMMONS_CODE" | jq -r '.total_loc // 0' 2>/dev/null || echo "0")
    COMMONS_TEST_LOC=$(echo "$COMMONS_TESTS" | jq -r '.test_loc // 0' 2>/dev/null || echo "0")
    COMMONS_TOTAL_LOC=$((COMMONS_PROD_LOC + COMMONS_TEST_LOC))
    
    # Build combined Commons JSON
    TEMP_COMMONS=$(mktemp)
    jq -n \
        --argjson code "$COMMONS_CODE" \
        --argjson features "$COMMONS_FEATURES" \
        --argjson tests "$COMMONS_TESTS" \
        --argjson prod_loc "$COMMONS_PROD_LOC" \
        --argjson test_loc "$COMMONS_TEST_LOC" \
        --argjson total_loc "$COMMONS_TOTAL_LOC" \
        '{
            production_code: $code,
            features: $features,
            tests: $tests,
            totals: {
                production_loc: $prod_loc,
                test_loc: $test_loc,
                total_loc: $total_loc,
                total_files: ($code.total_files // 0) + ($tests.test_files // 0)
            },
            breakdown: {
                production: $code,
                tests: $tests,
                features: $features
            }
        }' > "$TEMP_COMMONS" 2>/dev/null || echo '{}' > "$TEMP_COMMONS"
    
    jq --slurpfile commons_data "$TEMP_COMMONS" '.bitcoin_commons = $commons_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_COMMONS"
    
    echo "✅ Commons combined view: $COMMONS_TOTAL_LOC total LOC (prod: $COMMONS_PROD_LOC, test: $COMMONS_TEST_LOC)"
    
    # Calculate comparison
    if [ "$CORE_TOTAL_LOC" != "0" ] && [ "$COMMONS_TOTAL_LOC" != "0" ]; then
        TOTAL_RATIO=$(awk "BEGIN {printf \"%.2f\", $COMMONS_TOTAL_LOC / $CORE_TOTAL_LOC}" 2>/dev/null || echo "0")
        PROD_RATIO=$(awk "BEGIN {printf \"%.2f\", $COMMONS_PROD_LOC / $CORE_PROD_LOC}" 2>/dev/null || echo "0")
        TEST_RATIO=$(awk "BEGIN {printf \"%.2f\", $COMMONS_TEST_LOC / $CORE_TEST_LOC}" 2>/dev/null || echo "0")
        
        TEMP_COMP=$(mktemp)
        jq -n \
            --argjson total_ratio "$TOTAL_RATIO" \
            --argjson prod_ratio "$PROD_RATIO" \
            --argjson test_ratio "$TEST_RATIO" \
            --argjson core_total "$CORE_TOTAL_LOC" \
            --argjson commons_total "$COMMONS_TOTAL_LOC" \
            '{
                total_loc_ratio: $total_ratio,
                production_loc_ratio: $prod_ratio,
                test_loc_ratio: $test_ratio,
                core_total_loc: $core_total,
                commons_total_loc: $commons_total,
                analysis: "Combined view shows total codebase size including production code, tests, and feature-gated code. Ratios account for language differences."
            }' > "$TEMP_COMP" 2>/dev/null || echo '{}' > "$TEMP_COMP"
        
        jq --slurpfile comp_data "$TEMP_COMP" '.comparison = $comp_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
        rm -f "$TEMP_COMP"
    fi
else
    echo "⚠️  Missing required metrics files:"
    [ ! -f "$CODE_SIZE_FILE" ] && echo "  - Code size metrics not found"
    [ ! -f "$FEATURES_FILE" ] && echo "  - Features metrics not found"
    [ ! -f "$TESTS_FILE" ] && echo "  - Tests metrics not found"
    echo ""
    echo "⚠️  Run Phase 1 metrics first (code-size.sh, features.sh, tests.sh)"
fi

echo ""
echo "✅ Results saved to: $OUTPUT_FILE"
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

exit 0

