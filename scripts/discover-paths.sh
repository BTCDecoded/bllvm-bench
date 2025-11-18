#!/bin/bash
# Path Discovery for bllvm-bench
# Auto-discovers Bitcoin Core and Bitcoin Commons paths
# Can be sourced (. discover-paths.sh) or executed (./discover-paths.sh)

# Don't use set -e here as it can cause issues when sourced

# Load config if it exists
CONFIG_FILE="${BLLVM_BENCH_CONFIG:-./config/config.toml}"
if [ -f "$CONFIG_FILE" ]; then
    # Simple TOML parsing (basic key=value extraction)
    # Extract values, handling both quoted and unquoted strings
    CORE_PATH=$(grep -E "^core_path\s*=" "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\([^"]*\)".*/\1/' | sed 's/.*=\s*\([^#]*\).*/\1/' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' || echo "")
    COMMONS_CONSENSUS_PATH=$(grep -E "^commons_consensus_path\s*=" "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\([^"]*\)".*/\1/' | sed 's/.*=\s*\([^#]*\).*/\1/' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' || echo "")
    COMMONS_NODE_PATH=$(grep -E "^commons_node_path\s*=" "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\([^"]*\)".*/\1/' | sed 's/.*=\s*\([^#]*\).*/\1/' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' || echo "")
    
    # Resolve config paths to absolute paths
    if [ -n "$CORE_PATH" ] && [ -d "$CORE_PATH" ]; then
        CORE_PATH=$(cd "$CORE_PATH" 2>/dev/null && pwd || echo "$CORE_PATH")
    fi
    if [ -n "$COMMONS_CONSENSUS_PATH" ] && [ -d "$COMMONS_CONSENSUS_PATH" ]; then
        COMMONS_CONSENSUS_PATH=$(cd "$COMMONS_CONSENSUS_PATH" 2>/dev/null && pwd || echo "$COMMONS_CONSENSUS_PATH")
    fi
    if [ -n "$COMMONS_NODE_PATH" ] && [ -d "$COMMONS_NODE_PATH" ]; then
        COMMONS_NODE_PATH=$(cd "$COMMONS_NODE_PATH" 2>/dev/null && pwd || echo "$COMMONS_NODE_PATH")
    fi
fi

# Get script directory (bllvm-bench root)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BLLVM_BENCH_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# If BLLVM_BENCH_ROOT is already set and valid, use it
if [ -z "$BLLVM_BENCH_ROOT" ] || [ ! -d "$BLLVM_BENCH_ROOT" ]; then
    # Try to find bllvm-bench from current directory
    CURRENT_DIR="$(pwd)"
    if [ -f "$CURRENT_DIR/Cargo.toml" ] && grep -q "bllvm-bench" "$CURRENT_DIR/Cargo.toml" 2>/dev/null; then
        BLLVM_BENCH_ROOT="$CURRENT_DIR"
    elif [ -d "$CURRENT_DIR/scripts" ] && [ -f "$CURRENT_DIR/scripts/discover-paths.sh" ]; then
        BLLVM_BENCH_ROOT="$CURRENT_DIR"
    fi
fi

# Auto-discover Bitcoin Core
if [ -z "$CORE_PATH" ]; then
    # Common locations to search
    SEARCH_PATHS=(
        "$HOME/src/bitcoin"
        "$HOME/src/bitcoin-core"
        "$HOME/src/core"
        "../core"
        "../../core"
        "/usr/local/src/bitcoin"
        "/opt/bitcoin"
    )
    
    for path in "${SEARCH_PATHS[@]}"; do
        # Resolve to absolute path
        if [ -d "$path" ]; then
            abs_path=$(cd "$path" 2>/dev/null && pwd || echo "")
            if [ -n "$abs_path" ] && [ -f "$abs_path/src/CMakeLists.txt" ] && [ -f "$abs_path/src/bitcoin.cpp" ]; then
                CORE_PATH="$abs_path"
                break
            fi
        fi
    done
    
    # Also check if bench_bitcoin is in PATH
    if [ -z "$CORE_PATH" ] && command -v bench_bitcoin >/dev/null 2>&1; then
        BENCH_BITCOIN_PATH=$(command -v bench_bitcoin)
        potential_path=$(dirname "$(dirname "$(dirname "$BENCH_BITCOIN_PATH")")")
        if [ -d "$potential_path" ]; then
            CORE_PATH=$(cd "$potential_path" 2>/dev/null && pwd || echo "$potential_path")
        fi
    fi
fi

# Auto-discover Bitcoin Commons
if [ -z "$COMMONS_CONSENSUS_PATH" ] || [ -z "$COMMONS_NODE_PATH" ]; then
    # Common locations to search (relative to BLLVM_BENCH_ROOT and absolute)
    SEARCH_PATHS=(
        "$HOME/src/bllvm-consensus"
        "$HOME/src/bitcoin-commons"
        "../bllvm-consensus"
        "../../bllvm-consensus"
        "$BLLVM_BENCH_ROOT/../bllvm-consensus"
        "$BLLVM_BENCH_ROOT/../../bllvm-consensus"
        "$(dirname "$BLLVM_BENCH_ROOT")/bllvm-consensus"
        "$(dirname "$(dirname "$BLLVM_BENCH_ROOT")")/commons/bllvm-consensus"
        "$(dirname "$(dirname "$BLLVM_BENCH_ROOT")")/bllvm-consensus"
    )
    
    for path in "${SEARCH_PATHS[@]}"; do
        # Resolve to absolute path
        if [ -d "$path" ]; then
            abs_path=$(cd "$path" 2>/dev/null && pwd || echo "")
            if [ -n "$abs_path" ] && [ -f "$abs_path/Cargo.toml" ] && grep -q "bllvm-consensus" "$abs_path/Cargo.toml" 2>/dev/null; then
                COMMONS_CONSENSUS_PATH="$abs_path"
                # Try to find bllvm-node nearby
                NODE_CANDIDATES=(
                    "$(dirname "$abs_path")/bllvm-node"
                    "$(dirname "$(dirname "$abs_path")")/bllvm-node"
                    "$abs_path/../bllvm-node"
                )
                for node_path in "${NODE_CANDIDATES[@]}"; do
                    if [ -d "$node_path" ]; then
                        abs_node_path=$(cd "$node_path" 2>/dev/null && pwd || echo "")
                        if [ -n "$abs_node_path" ] && [ -f "$abs_node_path/Cargo.toml" ] && grep -q "bllvm-node" "$abs_node_path/Cargo.toml" 2>/dev/null; then
                            COMMONS_NODE_PATH="$abs_node_path"
                            break
                        fi
                    fi
                done
                break
            fi
        fi
    done
fi

# Export discovered paths
export CORE_PATH
export COMMONS_CONSENSUS_PATH
export COMMONS_NODE_PATH
export BLLVM_BENCH_ROOT

# Validate paths
if [ -n "$CORE_PATH" ] && [ ! -d "$CORE_PATH" ]; then
    echo "WARNING: CORE_PATH set but directory does not exist: ${CORE_PATH}" >&2
    CORE_PATH=""
fi

if [ -n "$COMMONS_CONSENSUS_PATH" ] && [ ! -d "$COMMONS_CONSENSUS_PATH" ]; then
    echo "WARNING: COMMONS_CONSENSUS_PATH set but directory does not exist: ${COMMONS_CONSENSUS_PATH}" >&2
    COMMONS_CONSENSUS_PATH=""
fi

if [ -n "$COMMONS_NODE_PATH" ] && [ ! -d "$COMMONS_NODE_PATH" ]; then
    echo "WARNING: COMMONS_NODE_PATH set but directory does not exist: ${COMMONS_NODE_PATH}" >&2
    COMMONS_NODE_PATH=""
fi

# Output paths (can be sourced by other scripts)
# Only output if script is executed directly (not sourced)
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    printf '%s\n' "export CORE_PATH=\"${CORE_PATH}\""
    printf '%s\n' "export COMMONS_CONSENSUS_PATH=\"${COMMONS_CONSENSUS_PATH}\""
    printf '%s\n' "export COMMONS_NODE_PATH=\"${COMMONS_NODE_PATH}\""
    printf '%s\n' "export BLLVM_BENCH_ROOT=\"${BLLVM_BENCH_ROOT}\""
fi
