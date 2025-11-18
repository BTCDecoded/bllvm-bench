#!/bin/bash
# Safe test script to validate all benchmarks can be discovered and would run
# This doesn't actually run benchmarks, just validates the setup

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Testing Benchmark Discovery (Dry Run)                        ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""

# Source common functions
if [ -f "scripts/shared/common.sh" ]; then
    source "scripts/shared/common.sh"
else
    echo "❌ Error: scripts/shared/common.sh not found"
    exit 1
fi

echo "Discovered paths:"
echo "  CORE_PATH: ${CORE_PATH:-NOT SET}"
echo "  COMMONS_CONSENSUS_PATH: ${COMMONS_CONSENSUS_PATH:-NOT SET}"
echo "  COMMONS_NODE_PATH: ${COMMONS_NODE_PATH:-NOT SET}"
echo ""

# Test Core benchmarks
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Core Benchmarks Discovery"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
CORE_COUNT=0
CORE_MISSING=0
for bench_script in "$BLLVM_BENCH_ROOT/scripts/core"/*.sh; do
    if [ -f "$bench_script" ]; then
        bench_name=$(basename "$bench_script" .sh)
        if [ -x "$bench_script" ]; then
            echo "  ✅ $bench_name"
            CORE_COUNT=$((CORE_COUNT + 1))
        else
            echo "  ⚠️  $bench_name (not executable)"
            CORE_MISSING=$((CORE_MISSING + 1))
        fi
    fi
done
echo "Total: $CORE_COUNT found, $CORE_MISSING issues"
echo ""

# Test Commons benchmarks
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Commons Benchmarks Discovery"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
COMMONS_COUNT=0
COMMONS_MISSING=0
for bench_script in "$BLLVM_BENCH_ROOT/scripts/commons"/*.sh; do
    if [ -f "$bench_script" ]; then
        bench_name=$(basename "$bench_script" .sh)
        if [ -x "$bench_script" ]; then
            echo "  ✅ $bench_name"
            COMMONS_COUNT=$((COMMONS_COUNT + 1))
        else
            echo "  ⚠️  $bench_name (not executable)"
            COMMONS_MISSING=$((COMMONS_MISSING + 1))
        fi
    fi
done
echo "Total: $COMMONS_COUNT found, $COMMONS_MISSING issues"
echo ""

# Check dependencies
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Dependency Check"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
MISSING_DEPS=0
for cmd in bash timeout jq git; do
    if command -v "$cmd" >/dev/null 2>&1; then
        echo "  ✅ $cmd"
    else
        echo "  ❌ $cmd (missing)"
        MISSING_DEPS=$((MISSING_DEPS + 1))
    fi
done
echo ""

# Summary
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Summary                                                      ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""
echo "Core benchmarks: $CORE_COUNT"
echo "Commons benchmarks: $COMMONS_COUNT"
echo "Total: $((CORE_COUNT + COMMONS_COUNT))"
echo "Missing dependencies: $MISSING_DEPS"
echo ""

if [ $MISSING_DEPS -eq 0 ] && [ $CORE_MISSING -eq 0 ] && [ $COMMONS_MISSING -eq 0 ]; then
    echo "✅ All checks passed! Ready to run benchmarks."
    exit 0
else
    echo "⚠️  Some issues found. Review above."
    exit 1
fi
