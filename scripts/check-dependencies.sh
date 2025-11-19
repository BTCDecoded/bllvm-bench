#!/bin/bash
# Check if all dependencies are available
# Quick validation before running benchmarks

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/shared/common.sh"

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Checking Dependencies                                       ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""

ERRORS=0

# Check required commands
for cmd in jq cargo rustc make; do
    if command -v "$cmd" >/dev/null 2>&1; then
        VERSION=$($cmd --version 2>/dev/null | head -1 || echo "installed")
        echo "✅ $cmd: $VERSION"
    else
        echo "❌ $cmd: Not found"
        ERRORS=$((ERRORS + 1))
    fi
done

echo ""

# Discover paths (source common.sh which includes discover-paths.sh)
# common.sh is already sourced, so paths should be available

# Check Bitcoin Core
if [ -n "$CORE_PATH" ]; then
    BENCH_BITCOIN=$(get_bench_bitcoin)
    if [ -n "$BENCH_BITCOIN" ] && [ -f "$BENCH_BITCOIN" ]; then
        echo "✅ Bitcoin Core: $CORE_PATH"
        echo "   bench_bitcoin: Found at $BENCH_BITCOIN"
    else
        echo "⚠️  Bitcoin Core: $CORE_PATH"
        echo "   bench_bitcoin: Not built (run: cd $CORE_PATH && make bench_bitcoin)"
        ERRORS=$((ERRORS + 1))
    fi
else
    echo "❌ Bitcoin Core: Not found"
    ERRORS=$((ERRORS + 1))
fi

# Check bllvm-consensus
if [ -n "$COMMONS_CONSENSUS_PATH" ] && [ -d "$COMMONS_CONSENSUS_PATH" ]; then
    echo "✅ bllvm-consensus: $COMMONS_CONSENSUS_PATH"
    if [ -f "$COMMONS_CONSENSUS_PATH/Cargo.toml" ]; then
        echo "   Cargo.toml: Found"
    else
        echo "   Cargo.toml: Missing"
        ERRORS=$((ERRORS + 1))
    fi
else
    echo "❌ bllvm-consensus: Not found"
    ERRORS=$((ERRORS + 1))
fi

# Check bllvm-node (optional)
if [ -n "$COMMONS_NODE_PATH" ] && [ -d "$COMMONS_NODE_PATH" ]; then
    echo "✅ bllvm-node: $COMMONS_NODE_PATH"
else
    echo "⚠️  bllvm-node: Not found (optional for some benchmarks)"
fi

echo ""

if [ $ERRORS -eq 0 ]; then
    echo "✅ All required dependencies found"
    exit 0
else
    echo "❌ Missing $ERRORS required dependency(ies)"
    echo ""
    echo "Run: make setup-auto  # To auto-clone missing dependencies"
    exit 1
fi

