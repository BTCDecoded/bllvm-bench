#!/bin/bash
# Feature Flags Analysis
# Analyzes feature flags in Core (CMake) and Commons (Cargo)

set +e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/shared/metrics-common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
OUTPUT_FILE="$OUTPUT_DIR/metrics-features-$(date +%Y%m%d-%H%M%S).json"

echo "=== Feature Flags Analysis ==="
echo ""

# Initialize JSON output
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "metric_type": "features",
  "bitcoin_core": {},
  "bitcoin_commons": {},
  "comparison": {}
}
EOF

# Collect Core features (from CMakeLists.txt)
if [ -n "$CORE_PATH" ] && [ -d "$CORE_PATH" ]; then
    echo "Analyzing Bitcoin Core features..."
    echo "  Core path: $CORE_PATH"
    
    # Look for CMakeLists.txt
    CMAKE_FILE="$CORE_PATH/CMakeLists.txt"
    CORE_FEATURES=()
    CORE_OPTIONS=()
    
    if [ -f "$CMAKE_FILE" ]; then
        # Extract option() and set() statements that look like features
        # This is a simplified extraction - CMake is complex
        while IFS= read -r line; do
            # Look for option() statements
            if echo "$line" | grep -qE '^\s*option\s*\('; then
                local opt_name=$(echo "$line" | sed -E 's/.*option\s*\(\s*([A-Z_]+).*/\1/')
                [ -n "$opt_name" ] && CORE_OPTIONS+=("$opt_name")
            fi
        done < "$CMAKE_FILE"
    fi
    
    # Count #ifdef blocks (feature gates)
    CORE_FEATURE_GATES=$(count_cpp_conditionals "$CORE_PATH/src")
    
    # Build JSON
    CORE_TOTAL_FEATURES=${#CORE_OPTIONS[@]}
    CORE_OPTIONS_JSON=$(printf '%s\n' "${CORE_OPTIONS[@]}" | jq -R . | jq -s . 2>/dev/null || echo "[]")
    
    TEMP_CORE=$(mktemp)
    jq -n \
        --argjson total_features "$CORE_TOTAL_FEATURES" \
        --argjson feature_gates "$CORE_FEATURE_GATES" \
        --argjson options "$CORE_OPTIONS_JSON" \
        '{
            total_features: $total_features,
            build_options: $options,
            conditional_blocks: $feature_gates,
            build_system: "CMake",
            note: "Core uses CMake build options and #ifdef/#if defined() for conditional compilation"
        }' > "$TEMP_CORE" 2>/dev/null || echo '{}' > "$TEMP_CORE"
    
    jq --slurpfile core_data "$TEMP_CORE" '.bitcoin_core = $core_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_CORE"
    
    echo "  ✅ Core features: $CORE_TOTAL_FEATURES build options, $CORE_FEATURE_GATES conditional blocks"
else
    echo "⚠️  Bitcoin Core path not found, skipping Core features"
fi

# Collect Commons features (from Cargo.toml)
COMMONS_TOTAL_FEATURES=0
COMMONS_TOTAL_GATES=0
COMMONS_CRATES_FEATURES="{}"

if [ -n "$COMMONS_CONSENSUS_PATH" ] && [ -d "$COMMONS_CONSENSUS_PATH" ]; then
    echo "Analyzing Bitcoin Commons features..."
    
    # bllvm-consensus
    CONSENSUS_CARGO="$COMMONS_CONSENSUS_PATH/Cargo.toml"
    if [ -f "$CONSENSUS_CARGO" ]; then
        echo "  Analyzing bllvm-consensus features..."
        CONSENSUS_FEATURES=$(parse_cargo_features "$CONSENSUS_CARGO")
        CONSENSUS_FEATURES_COUNT=$(echo "$CONSENSUS_FEATURES" | jq -r '.total_features // 0' 2>/dev/null || echo "0")
        CONSENSUS_GATES=$(count_rust_feature_gates "$COMMONS_CONSENSUS_PATH/src")
        
        COMMONS_TOTAL_FEATURES=$((COMMONS_TOTAL_FEATURES + CONSENSUS_FEATURES_COUNT))
        COMMONS_TOTAL_GATES=$((COMMONS_TOTAL_GATES + CONSENSUS_GATES))
        
        COMMONS_CRATES_FEATURES=$(echo "$COMMONS_CRATES_FEATURES" | jq --argjson features "$CONSENSUS_FEATURES" --argjson gates "$CONSENSUS_GATES" \
            '. + {"bllvm-consensus": ($features + {"feature_gates": $gates})}' 2>/dev/null || echo "$COMMONS_CRATES_FEATURES")
        
        echo "    bllvm-consensus: $CONSENSUS_FEATURES_COUNT features, $CONSENSUS_GATES feature gates"
    fi
fi

if [ -n "$COMMONS_NODE_PATH" ] && [ -d "$COMMONS_NODE_PATH" ]; then
    # bllvm-node
    NODE_CARGO="$COMMONS_NODE_PATH/Cargo.toml"
    if [ -f "$NODE_CARGO" ]; then
        echo "  Analyzing bllvm-node features..."
        NODE_FEATURES=$(parse_cargo_features "$NODE_CARGO")
        NODE_FEATURES_COUNT=$(echo "$NODE_FEATURES" | jq -r '.total_features // 0' 2>/dev/null || echo "0")
        NODE_GATES=$(count_rust_feature_gates "$COMMONS_NODE_PATH/src")
        
        COMMONS_TOTAL_FEATURES=$((COMMONS_TOTAL_FEATURES + NODE_FEATURES_COUNT))
        COMMONS_TOTAL_GATES=$((COMMONS_TOTAL_GATES + NODE_GATES))
        
        COMMONS_CRATES_FEATURES=$(echo "$COMMONS_CRATES_FEATURES" | jq --argjson features "$NODE_FEATURES" --argjson gates "$NODE_GATES" \
            '. + {"bllvm-node": ($features + {"feature_gates": $gates})}' 2>/dev/null || echo "$COMMONS_CRATES_FEATURES")
        
        echo "    bllvm-node: $NODE_FEATURES_COUNT features, $NODE_GATES feature gates"
    fi
fi

if [ "$COMMONS_TOTAL_FEATURES" -gt 0 ]; then
    # Update JSON with Commons data
    TEMP_COMMONS=$(mktemp)
    jq -n \
        --argjson total_features "$COMMONS_TOTAL_FEATURES" \
        --argjson total_gates "$COMMONS_TOTAL_GATES" \
        --argjson crates "$COMMONS_CRATES_FEATURES" \
        '{
            total_features: $total_features,
            total_feature_gates: $total_gates,
            by_crate: $crates,
            build_system: "Cargo",
            note: "Commons uses Cargo features and #[cfg(feature = \"...\")] for conditional compilation"
        }' > "$TEMP_COMMONS" 2>/dev/null || echo '{}' > "$TEMP_COMMONS"
    
    jq --slurpfile commons_data "$TEMP_COMMONS" '.bitcoin_commons = $commons_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_COMMONS"
    
    echo "  ✅ Commons features: $COMMONS_TOTAL_FEATURES total features, $COMMONS_TOTAL_GATES feature gates"
else
    echo "⚠️  Bitcoin Commons paths not found, skipping Commons features"
fi

# Calculate comparison
CORE_FEATURES_VAL=$(jq -r '.bitcoin_core.total_features // 0' "$OUTPUT_FILE" 2>/dev/null || echo "0")
COMMONS_FEATURES_VAL=$(jq -r '.bitcoin_commons.total_features // 0' "$OUTPUT_FILE" 2>/dev/null || echo "0")

if [ "$CORE_FEATURES_VAL" != "0" ] && [ "$COMMONS_FEATURES_VAL" != "0" ]; then
    FEATURES_DELTA=$((COMMONS_FEATURES_VAL - CORE_FEATURES_VAL))
    
    TEMP_COMP=$(mktemp)
    jq -n \
        --argjson delta "$FEATURES_DELTA" \
        --argjson core "$CORE_FEATURES_VAL" \
        --argjson commons "$COMMONS_FEATURES_VAL" \
        '{
            feature_count_delta: $delta,
            core_features: $core,
            commons_features: $commons,
            analysis: "Feature flag systems differ: Core uses CMake options, Commons uses Cargo features. Direct comparison may not be meaningful."
        }' > "$TEMP_COMP" 2>/dev/null || echo '{}' > "$TEMP_COMP"
    
    jq --slurpfile comp_data "$TEMP_COMP" '.comparison = $comp_data[0]' "$OUTPUT_FILE" > "$OUTPUT_FILE.tmp" && mv "$OUTPUT_FILE.tmp" "$OUTPUT_FILE"
    rm -f "$TEMP_COMP"
fi

echo ""
echo "✅ Results saved to: $OUTPUT_FILE"
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

exit 0

