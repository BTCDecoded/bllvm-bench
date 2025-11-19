#!/bin/bash
# Common functions and path setup for bllvm-bench scripts
# This file should be sourced by all benchmark scripts

# Don't use set -e here - we want functions to be defined even if path discovery fails
# Individual scripts can use set -e if they want
set +e

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Find bllvm-bench root (could be scripts/../.. or scripts/..)
# Don't override if already set (e.g., by run-benchmarks.sh)
if [ -z "$BLLVM_BENCH_ROOT" ]; then
    if [ -f "$SCRIPT_DIR/../discover-paths.sh" ]; then
        BLLVM_BENCH_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
    elif [ -f "$SCRIPT_DIR/../../scripts/discover-paths.sh" ]; then
        BLLVM_BENCH_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
    else
        # Try to find it from current working directory
        CURRENT_DIR="$(pwd)"
        if [ -f "$CURRENT_DIR/scripts/discover-paths.sh" ]; then
            BLLVM_BENCH_ROOT="$CURRENT_DIR"
        elif [ -f "$CURRENT_DIR/discover-paths.sh" ]; then
            BLLVM_BENCH_ROOT="$CURRENT_DIR"
        else
            BLLVM_BENCH_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
        fi
    fi
fi

# Discover paths
DISCOVER_SCRIPT="$BLLVM_BENCH_ROOT/scripts/discover-paths.sh"
if [ ! -f "$DISCOVER_SCRIPT" ]; then
    DISCOVER_SCRIPT="$BLLVM_BENCH_ROOT/discover-paths.sh"
fi

if [ -f "$DISCOVER_SCRIPT" ]; then
    # Source the path discovery - use source instead of eval to avoid quote issues
    # Temporarily disable set -e to allow script to complete even if paths not found
    set +e
    . "$DISCOVER_SCRIPT"
    set -e
else
    echo "⚠️  Warning: discover-paths.sh not found at $DISCOVER_SCRIPT" >&2
    echo "   BLLVM_BENCH_ROOT: $BLLVM_BENCH_ROOT" >&2
    echo "   SCRIPT_DIR: $SCRIPT_DIR" >&2
fi

# Validate required paths (allow Core-only or Commons-only runs)
# Only exit if BOTH are missing (at least one must be available)
if [ -z "$COMMONS_CONSENSUS_PATH" ] && [ -z "$CORE_PATH" ]; then
    echo "❌ Error: Neither Commons nor Core paths discovered" >&2
    echo "   BLLVM_BENCH_ROOT: ${BLLVM_BENCH_ROOT:-NOT SET}" >&2
    echo "   Current directory: $(pwd)" >&2
    echo "   CORE_PATH: ${CORE_PATH:-NOT SET}" >&2
    echo "   COMMONS_CONSENSUS_PATH: ${COMMONS_CONSENSUS_PATH:-NOT SET}" >&2
    echo "   COMMONS_NODE_PATH: ${COMMONS_NODE_PATH:-NOT SET}" >&2
    echo "   BLLVM_BENCH_CONFIG: ${BLLVM_BENCH_CONFIG:-NOT SET}" >&2
    echo "   Please set paths in config/config.toml or ensure Core/Commons are in standard locations" >&2
    echo "" >&2
    echo "   Trying manual discovery..." >&2
    # Last resort: try to find it from common locations
    for path in "../bllvm-consensus" "../../bllvm-consensus" "$HOME/src/bllvm-consensus" "$HOME/bllvm-consensus" "$(dirname "$BLLVM_BENCH_ROOT")/bllvm-consensus" "$(dirname "$(dirname "$BLLVM_BENCH_ROOT")")/commons/bllvm-consensus"; do
        if [ -d "$path" ] && [ -f "$path/Cargo.toml" ] && grep -q "bllvm-consensus" "$path/Cargo.toml" 2>/dev/null; then
            abs_path=$(cd "$path" 2>/dev/null && pwd || echo "")
            if [ -n "$abs_path" ]; then
                COMMONS_CONSENSUS_PATH="$abs_path"
                export COMMONS_CONSENSUS_PATH
                echo "   ✅ Found: $COMMONS_CONSENSUS_PATH" >&2
                # Also try to find bllvm-node
                for node_path in "$(dirname "$abs_path")/bllvm-node" "$abs_path/../bllvm-node" "$HOME/bllvm-node"; do
                    if [ -d "$node_path" ] && [ -f "$node_path/Cargo.toml" ] && grep -q "bllvm-node" "$node_path/Cargo.toml" 2>/dev/null; then
                        abs_node_path=$(cd "$node_path" 2>/dev/null && pwd || echo "")
                        if [ -n "$abs_node_path" ]; then
                            COMMONS_NODE_PATH="$abs_node_path"
                            export COMMONS_NODE_PATH
                            echo "   ✅ Found node: $COMMONS_NODE_PATH" >&2
                            break
                        fi
                    fi
                done
                break
            fi
        fi
    done
    
    # Only exit if STILL both are missing after manual discovery
    if [ -z "$COMMONS_CONSENSUS_PATH" ] && [ -z "$CORE_PATH" ]; then
        echo "   ❌ Failed to find either Core or Commons in any standard location" >&2
        exit 1
    fi
elif [ -z "$COMMONS_CONSENSUS_PATH" ]; then
    # Commons not found but Core is available - this is OK for Core-only benchmarks
    echo "⚠️  Warning: Commons consensus path not discovered (Core-only mode)" >&2
fi

# Set up results directory (always define, even if BLLVM_BENCH_ROOT is not set)
RESULTS_DIR="${RESULTS_DIR:-${BLLVM_BENCH_ROOT:-$(pwd)}/results}"
mkdir -p "$RESULTS_DIR" 2>/dev/null || true

# Helper function to get output directory (from first argument or default)
# This function MUST be defined even if path discovery failed
get_output_dir() {
    local output_dir="${1:-$RESULTS_DIR}"
    # Try to resolve to absolute path, fallback to current directory/results
    output_dir=$(cd "$output_dir" 2>/dev/null && pwd || echo "${RESULTS_DIR:-$(pwd)/results}")
    mkdir -p "$output_dir" 2>/dev/null || true
    echo "$output_dir"
}

# Function to reliably find or build bench_bitcoin
# This function MUST be defined even if path discovery failed
get_bench_bitcoin() {
    local bench_bitcoin_path=""
    
    if [ -n "$CORE_PATH" ] && [ -d "$CORE_PATH" ]; then
        # Try build/bin/bench_bitcoin (CMake default)
        if [ -f "$CORE_PATH/build/bin/bench_bitcoin" ]; then
            bench_bitcoin_path="$CORE_PATH/build/bin/bench_bitcoin"
        # Try src/bench_bitcoin (autotools default)
        elif [ -f "$CORE_PATH/src/bench_bitcoin" ]; then
            bench_bitcoin_path="$CORE_PATH/src/bench_bitcoin"
        # Try bin/bench_bitcoin (alternative)
        elif [ -f "$CORE_PATH/bin/bench_bitcoin" ]; then
            bench_bitcoin_path="$CORE_PATH/bin/bench_bitcoin"
        # Try build/src/bench_bitcoin (alternative CMake)
        elif [ -f "$CORE_PATH/build/src/bench_bitcoin" ]; then
            bench_bitcoin_path="$CORE_PATH/build/src/bench_bitcoin"
        fi
    fi

    # Fallback: check if bench_bitcoin is in PATH
    if [ -z "$bench_bitcoin_path" ] && command -v bench_bitcoin >/dev/null 2>&1; then
        bench_bitcoin_path=$(command -v bench_bitcoin)
    fi

    if [ -z "$bench_bitcoin_path" ] || [ ! -f "$bench_bitcoin_path" ]; then
        if [ -n "$CORE_PATH" ]; then
            echo "❌ bench_bitcoin not found at expected paths." >&2
            echo "   Attempting to build bench_bitcoin in $CORE_PATH..." >&2
            (
                cd "$CORE_PATH" || exit 1
                echo "Current directory: $(pwd)" >&2
                
                # Try CMake build
                if [ -d "build" ] && [ -f "build/CMakeCache.txt" ]; then
                    echo "Attempting CMake build..." >&2
                    cmake --build build -t bench_bitcoin -j$(nproc) 2>&1 | tail -10 >&2 || true
                elif [ -f "CMakeLists.txt" ]; then
                    echo "Attempting CMake configure and build..." >&2
                    mkdir -p build
                    cmake -B build -DCMAKE_BUILD_TYPE=Release -DBUILD_BENCH=ON -DENABLE_WALLET=OFF -DBUILD_GUI=OFF 2>&1 | tail -10 >&2 || true
                    cmake --build build -t bench_bitcoin -j$(nproc) 2>&1 | tail -10 >&2 || true
                # Fallback to autotools build
                elif [ -f "autogen.sh" ]; then
                    echo "Attempting autotools build..." >&2
                    ./autogen.sh 2>&1 | tail -10 >&2 || true
                    ./configure --enable-bench --disable-wallet --disable-gui 2>&1 | tail -10 >&2 || true
                    make -j$(nproc) bench_bitcoin 2>&1 | tail -10 >&2 || true
                else
                    echo "⚠️  Could not find build system (CMake or autotools) in $CORE_PATH" >&2
                fi
            )
            
            # Re-check after attempted build
            if [ -f "$CORE_PATH/build/bin/bench_bitcoin" ]; then
                bench_bitcoin_path="$CORE_PATH/build/bin/bench_bitcoin"
            elif [ -f "$CORE_PATH/src/bench_bitcoin" ]; then
                bench_bitcoin_path="$CORE_PATH/src/bench_bitcoin"
            elif [ -f "$CORE_PATH/bin/bench_bitcoin" ]; then
                bench_bitcoin_path="$CORE_PATH/bin/bench_bitcoin"
            elif [ -f "$CORE_PATH/build/src/bench_bitcoin" ]; then
                bench_bitcoin_path="$CORE_PATH/build/src/bench_bitcoin"
            fi
        fi
    fi

    if [ -n "$bench_bitcoin_path" ] && [ -f "$bench_bitcoin_path" ]; then
        echo "$bench_bitcoin_path"
    else
        echo "" # Return empty if not found/built
    fi
}

# Helper function to format time
format_time() {
    local time_value="$1"
    if [ -z "$time_value" ] || [ "$time_value" = "null" ] || [ "$time_value" = "0" ]; then
        echo ""
        return
    fi
    
    # Convert to appropriate unit
    if awk "BEGIN {exit !($time_value >= 1000)}" 2>/dev/null; then
        # >= 1000 ms, show as seconds
        awk "BEGIN {printf \"%.3f s\", $time_value / 1000}" 2>/dev/null || echo "${time_value} ms"
    elif awk "BEGIN {exit !($time_value >= 1)}" 2>/dev/null; then
        # >= 1 ms, show as milliseconds
        awk "BEGIN {printf \"%.2f ms\", $time_value}" 2>/dev/null || echo "${time_value} ms"
    elif awk "BEGIN {exit !($time_value >= 0.001)}" 2>/dev/null; then
        # >= 0.001 ms (1 µs), show as microseconds
        awk "BEGIN {printf \"%.2f µs\", $time_value * 1000}" 2>/dev/null || echo "${time_value} ms"
    else
        # < 1 µs, show as nanoseconds
        awk "BEGIN {printf \"%.2f ns\", $time_value * 1000000}" 2>/dev/null || echo "${time_value} ms"
    fi
}

    # Helper function to safely create JSON numbers for jq
    # Usage: safe_json_number <value> [default]
    # Returns: Valid JSON number string
    safe_json_number() {
        local value="${1:-0}"
        local default="${2:-0}"
        # Validate and format number
        local num=$(awk "BEGIN {printf \"%.10g\", ($value + 0)}" 2>/dev/null || echo "$default")
        # Ensure it's a valid number (not NaN or inf)
        if echo "$num" | grep -qE '^-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?$'; then
            echo "$num"
        else
            echo "$default"
        fi
    }
    
    # Helper function to safely use numbers in jq (replaces --argjson)
    # Usage: jq_with_number <jq_expr> <number_var> [number_value]
    # This creates a temp file with the number and uses --slurpfile
    jq_with_safe_number() {
        local jq_expr="$1"
        local number_var="$2"
        local number_value="${3:-0}"
        
        # Validate number
        local safe_num=$(safe_json_number "$number_value" "0")
        
        # Create temp file with JSON number
        local temp_file=$(mktemp)
        echo "$safe_num" > "$temp_file"
        
        # Use --slurpfile to read it
        local result=$(jq --slurpfile "$number_var" "$temp_file" "$jq_expr" 2>/dev/null)
        rm -f "$temp_file"
        echo "$result"
    }

    # Export common variables
    export CORE_PATH
    export COMMONS_CONSENSUS_PATH
    export COMMONS_NODE_PATH
    export BLLVM_BENCH_ROOT
    export RESULTS_DIR

