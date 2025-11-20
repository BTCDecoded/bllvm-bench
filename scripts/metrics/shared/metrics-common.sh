#!/bin/bash
# Common functions for codebase metrics collection
# Provides shared utilities for all metrics scripts

# Don't exit on error - we want to write JSON even if collection fails
set +e

# Source the main common.sh for path discovery
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BLLVM_BENCH_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
source "$BLLVM_BENCH_ROOT/scripts/shared/common.sh"

# Ensure paths are exported
export CORE_PATH
export COMMONS_CONSENSUS_PATH
export COMMONS_NODE_PATH
export BLLVM_BENCH_ROOT

# Check if tokei is available
check_tokei() {
    if command -v tokei >/dev/null 2>&1; then
        return 0
    else
        echo "⚠️  tokei not found, attempting to install..." >&2
        # Try cargo install (if Rust is available)
        if command -v cargo >/dev/null 2>&1; then
            cargo install tokei --locked 2>/dev/null || {
                echo "⚠️  cargo install tokei failed" >&2
                return 1
            }
            return 0
        else
            echo "⚠️  cargo not available, cannot install tokei" >&2
            return 1
        fi
    fi
}

# Get code size using tokei (preferred) or fallback
get_code_size() {
    local target_dir="$1"
    local output_format="${2:-json}"
    
    if [ ! -d "$target_dir" ]; then
        echo "{}"
        return 1
    fi
    
    if check_tokei; then
        # Use tokei with JSON output
        tokei --output "$output_format" "$target_dir" 2>/dev/null || {
            echo "⚠️  tokei failed, using fallback" >&2
            get_code_size_fallback "$target_dir"
        }
    else
        get_code_size_fallback "$target_dir"
    fi
}

# Fallback code size calculation using wc
get_code_size_fallback() {
    local target_dir="$1"
    
    # Count lines in source files
    local cpp_files=$(find "$target_dir" -name "*.cpp" -o -name "*.h" 2>/dev/null | wc -l)
    local rs_files=$(find "$target_dir" -name "*.rs" 2>/dev/null | wc -l)
    local total_loc=$(find "$target_dir" -type f \( -name "*.cpp" -o -name "*.h" -o -name "*.rs" \) 2>/dev/null | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1}' || echo "0")
    
    # Create simple JSON structure
    cat <<EOF
{
  "total": $total_loc,
  "files": {
    "cpp": $cpp_files,
    "rust": $rs_files
  }
}
EOF
}

# Parse Cargo.toml for features
parse_cargo_features() {
    local cargo_toml="$1"
    
    if [ ! -f "$cargo_toml" ]; then
        echo "{}"
        return 1
    fi
    
    # Extract features section
    local in_features=false
    local features=()
    local default_features=()
    
    while IFS= read -r line; do
        # Check if we're in [features] section
        if echo "$line" | grep -qE '^\s*\[features\]'; then
            in_features=true
            continue
        fi
        
        # Check if we've left features section
        if [ "$in_features" = "true" ] && echo "$line" | grep -qE '^\s*\['; then
            break
        fi
        
        # Extract feature names
        if [ "$in_features" = "true" ]; then
            # Check for default features
            if echo "$line" | grep -qE '^\s*default\s*='; then
                # Extract default features
                local defaults=$(echo "$line" | sed -E 's/.*=\s*\[(.*)\].*/\1/' | tr -d '"' | tr ',' '\n' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
                while IFS= read -r feat; do
                    [ -n "$feat" ] && default_features+=("$feat")
                done <<< "$defaults"
            elif echo "$line" | grep -qE '^\s*[a-zA-Z0-9_-]+\s*=' && ! echo "$line" | grep -qE '^\s*default\s*='; then
                # Extract feature name (before =)
                local feat_name=$(echo "$line" | sed -E 's/^\s*([a-zA-Z0-9_-]+)\s*=.*/\1/')
                [ -n "$feat_name" ] && features+=("$feat_name")
            fi
        fi
    done < "$cargo_toml"
    
    # Build JSON output
    local features_json=$(printf '%s\n' "${features[@]}" | jq -R . | jq -s . 2>/dev/null || echo "[]")
    local default_json=$(printf '%s\n' "${default_features[@]}" | jq -R . | jq -s . 2>/dev/null || echo "[]")
    
    cat <<EOF
{
  "total_features": ${#features[@]},
  "default_features": ${#default_features[@]},
  "optional_features": $((${#features[@]} - ${#default_features[@]})),
  "features": $features_json,
  "default": $default_json
}
EOF
}

# Count #[cfg(feature = "...")] blocks in Rust code
count_rust_feature_gates() {
    local target_dir="$1"
    
    local cfg_count=$(find "$target_dir" -name "*.rs" -type f 2>/dev/null | xargs grep -hE '#\[cfg\(feature\s*=' 2>/dev/null | wc -l || echo "0")
    local cfg_any_count=$(find "$target_dir" -name "*.rs" -type f 2>/dev/null | xargs grep -hE '#\[cfg\(any\(' 2>/dev/null | wc -l || echo "0")
    local cfg_all_count=$(find "$target_dir" -name "*.rs" -type f 2>/dev/null | xargs grep -hE '#\[cfg\(all\(' 2>/dev/null | wc -l || echo "0")
    
    local total=$((cfg_count + cfg_any_count + cfg_all_count))
    
    echo "$total"
}

# Count #ifdef / #if defined() blocks in C++ code
count_cpp_conditionals() {
    local target_dir="$1"
    
    local ifdef_count=$(find "$target_dir" -name "*.cpp" -o -name "*.h" 2>/dev/null | xargs grep -hE '^\s*#ifdef|^\s*#if\s+defined' 2>/dev/null | wc -l || echo "0")
    
    echo "$ifdef_count"
}

# Get module/crate breakdown
get_module_breakdown() {
    local target_dir="$1"
    local codebase_type="${2:-unknown}"  # "core" or "commons"
    
    local breakdown="{}"
    
    if [ "$codebase_type" = "core" ] && [ -d "$target_dir/src" ]; then
        # For Core, break down by src/ subdirectories
        local modules_json="{}"
        for module_dir in "$target_dir/src"/*; do
            if [ -d "$module_dir" ]; then
                local module_name=$(basename "$module_dir")
                local module_loc=$(get_code_size "$module_dir" json | jq -r '.total // 0' 2>/dev/null || echo "0")
                local module_files=$(find "$module_dir" -type f \( -name "*.cpp" -o -name "*.h" \) 2>/dev/null | wc -l)
                
                if [ "$module_loc" != "0" ] || [ "$module_files" != "0" ]; then
                    modules_json=$(echo "$modules_json" | jq --arg name "$module_name" --argjson loc "$module_loc" --argjson files "$module_files" \
                        '. + {($name): {"loc": $loc, "files": $files}}' 2>/dev/null || echo "$modules_json")
                fi
            fi
        done
        breakdown="$modules_json"
    elif [ "$codebase_type" = "commons" ]; then
        # For Commons, we're analyzing individual crates (bllvm-consensus, bllvm-node)
        # Each crate is passed separately, so just return its stats
        local crate_loc=$(get_code_size "$target_dir" json | jq -r '.total // 0' 2>/dev/null || echo "0")
        local crate_files=$(find "$target_dir/src" -name "*.rs" 2>/dev/null | wc -l)
        breakdown=$(jq -n --argjson loc "$crate_loc" --argjson files "$crate_files" \
            '{"loc": $loc, "files": $files}' 2>/dev/null || echo "{}")
    fi
    
    echo "$breakdown"
}

