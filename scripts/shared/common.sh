#!/bin/bash
# Common functions and path setup for bllvm-bench scripts
# This file should be sourced by all benchmark scripts

set -e

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

# Validate required paths (allow Commons-only runs)
if [ -z "$COMMONS_CONSENSUS_PATH" ]; then
    echo "❌ Error: Commons consensus path not discovered" >&2
    echo "   BLLVM_BENCH_ROOT: ${BLLVM_BENCH_ROOT:-NOT SET}" >&2
    echo "   Current directory: $(pwd)" >&2
    echo "   CORE_PATH: ${CORE_PATH:-NOT SET}" >&2
    echo "   COMMONS_CONSENSUS_PATH: ${COMMONS_CONSENSUS_PATH:-NOT SET}" >&2
    echo "   COMMONS_NODE_PATH: ${COMMONS_NODE_PATH:-NOT SET}" >&2
    echo "   BLLVM_BENCH_CONFIG: ${BLLVM_BENCH_CONFIG:-NOT SET}" >&2
    echo "   Please set paths in config/config.toml or ensure Commons are in standard locations" >&2
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
    
    if [ -z "$COMMONS_CONSENSUS_PATH" ]; then
        echo "   ❌ Failed to find bllvm-consensus in any standard location" >&2
        exit 1
    fi
fi

# Set up results directory
RESULTS_DIR="${RESULTS_DIR:-$BLLVM_BENCH_ROOT/results}"
mkdir -p "$RESULTS_DIR"

# Helper function to get output directory (from first argument or default)
get_output_dir() {
    local output_dir="${1:-$RESULTS_DIR}"
    output_dir=$(cd "$output_dir" 2>/dev/null && pwd || echo "$(cd "$(dirname "$0")/../.." && pwd)/results")
    mkdir -p "$output_dir"
    echo "$output_dir"
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

# Export common variables
export CORE_PATH
export COMMONS_CONSENSUS_PATH
export COMMONS_NODE_PATH
export BLLVM_BENCH_ROOT
export RESULTS_DIR

