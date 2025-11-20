#!/bin/bash
# Update all Core benchmark scripts to use correct benchmark names
# This script reads the detected benchmarks and updates filter patterns

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BLLVM_BENCH_ROOT="${BLLVM_BENCH_ROOT:-$(cd "$SCRIPT_DIR/../.." && pwd)}"
BENCHMARKS_JSON="$BLLVM_BENCH_ROOT/scripts/shared/bench_bitcoin_benchmarks.json"

if [ ! -f "$BENCHMARKS_JSON" ]; then
    echo "❌ Benchmark mapping not found: $BENCHMARKS_JSON" >&2
    echo "   Run detect-bench-bitcoin-benchmarks.sh first" >&2
    exit 1
fi

echo "🔄 Updating Core benchmark scripts with detected benchmark names..."
echo ""

# Load benchmark mappings
MEMPOOL_BENCHMARKS=$(jq -r '.categories.mempool[]?' "$BENCHMARKS_JSON" 2>/dev/null | grep -v null || echo "")
TRANSACTION_BENCHMARKS=$(jq -r '.categories.transaction[]?' "$BENCHMARKS_JSON" 2>/dev/null | grep -v null || echo "")
BLOCK_BENCHMARKS=$(jq -r '.categories.block[]?' "$BENCHMARKS_JSON" 2>/dev/null | grep -v null || echo "")
SCRIPT_BENCHMARKS=$(jq -r '.categories.script[]?' "$BENCHMARKS_JSON" 2>/dev/null | grep -v null || echo "")
HASH_BENCHMARKS=$(jq -r '.categories.hash[]?' "$BENCHMARKS_JSON" 2>/dev/null | grep -v null || echo "")
ENCODING_BENCHMARKS=$(jq -r '.categories.encoding[]?' "$BENCHMARKS_JSON" 2>/dev/null | grep -v null || echo "")
UTXO_BENCHMARKS=$(jq -r '.categories.utxo[]?' "$BENCHMARKS_JSON" 2>/dev/null | grep -v null || echo "")

# Build filter patterns (pipe-separated for -filter flag)
MEMPOOL_FILTER=$(echo "$MEMPOOL_BENCHMARKS" | tr '\n' '|' | sed 's/|$//')
TRANSACTION_FILTER=$(echo "$TRANSACTION_BENCHMARKS" | tr '\n' '|' | sed 's/|$//')
BLOCK_FILTER=$(echo "$BLOCK_BENCHMARKS" | tr '\n' '|' | sed 's/|$//')
SCRIPT_FILTER=$(echo "$SCRIPT_BENCHMARKS" | tr '\n' '|' | sed 's/|$//')
HASH_FILTER=$(echo "$HASH_BENCHMARKS" | tr '\n' '|' | sed 's/|$//')
ENCODING_FILTER=$(echo "$ENCODING_BENCHMARKS" | tr '\n' '|' | sed 's/|$//')
UTXO_FILTER=$(echo "$UTXO_BENCHMARKS" | tr '\n' '|' | sed 's/|$//')

echo "📋 Detected filters:"
echo "  Mempool: ${MEMPOOL_FILTER:-none}"
echo "  Transaction: ${TRANSACTION_FILTER:-none}"
echo "  Block: ${BLOCK_FILTER:-none}"
echo "  Script: ${SCRIPT_FILTER:-none}"
echo "  Hash: ${HASH_FILTER:-none}"
echo "  Encoding: ${ENCODING_FILTER:-none}"
echo "  UTXO: ${UTXO_FILTER:-none}"
echo ""

# Update mempool-operations-bench.sh
if [ -n "$MEMPOOL_FILTER" ] && [ -f "$BLLVM_BENCH_ROOT/scripts/core/mempool-operations-bench.sh" ]; then
    echo "✅ Updating mempool-operations-bench.sh"
    # Escape special characters for sed
    ESCAPED_FILTER=$(echo "$MEMPOOL_FILTER" | sed 's/[[\.*^$()+?{|]/\\&/g')
    # Update the filter line (use different delimiter to avoid conflicts)
    sed -i "s|-filter=\"[^\"]*\"|-filter=\"$MEMPOOL_FILTER\"|g" "$BLLVM_BENCH_ROOT/scripts/core/mempool-operations-bench.sh"
    # Update grep patterns (escape properly)
    GREP_PATTERN=$(echo "$MEMPOOL_BENCHMARKS" | tr '\n' '|' | sed 's/|$//' | sed 's/[[\.*^$()+?{|]/\\&/g')
    sed -i "s|grep -oE \"(MempoolCheck|MempoolEviction|MempoolAccept)\"|grep -oE \"($GREP_PATTERN)\"|g" "$BLLVM_BENCH_ROOT/scripts/core/mempool-operations-bench.sh"
    # Update grep -qE pattern (use different delimiter)
    ESCAPED_GREP=$(echo "$MEMPOOL_BENCHMARKS" | tr '\n' '|' | sed 's/|$//' | sed 's/[[\.*^$()+?{|]/\\&/g')
    sed -i "s|grep -qE '.*MempoolCheck|MempoolEviction|MempoolAccept'|grep -qE '.*$ESCAPED_GREP'|g" "$BLLVM_BENCH_ROOT/scripts/core/mempool-operations-bench.sh"
fi

# Update transaction benchmarks
if [ -n "$TRANSACTION_FILTER" ]; then
    for script in transaction-serialization-bench.sh transaction-id-bench.sh transaction-sighash-bench.sh; do
        if [ -f "$BLLVM_BENCH_ROOT/scripts/core/$script" ]; then
            echo "✅ Updating $script"
            # Extract the specific benchmark name for each script
            case "$script" in
                transaction-serialization-bench.sh)
                    BENCH_NAME=$(echo "$TRANSACTION_BENCHMARKS" | grep -i "serialization" | head -1 || echo "TransactionSerialization")
                    sed -i "s|-filter=\"[^\"]*\"|-filter=\"$BENCH_NAME\"|g" "$BLLVM_BENCH_ROOT/scripts/core/$script"
                    ;;
                transaction-id-bench.sh)
                    BENCH_NAME=$(echo "$TRANSACTION_BENCHMARKS" | grep -i "id\|txid" | head -1 || echo "TransactionIdCalculation")
                    sed -i "s|-filter=\"[^\"]*\"|-filter=\"$BENCH_NAME\"|g" "$BLLVM_BENCH_ROOT/scripts/core/$script"
                    ;;
                transaction-sighash-bench.sh)
                    BENCH_NAME=$(echo "$TRANSACTION_BENCHMARKS" | grep -i "sighash" | head -1 || echo "TransactionSighashCalculation")
                    sed -i "s|-filter=\"[^\"]*\"|-filter=\"$BENCH_NAME\"|g" "$BLLVM_BENCH_ROOT/scripts/core/$script"
                    ;;
            esac
        fi
    done
fi

# Update block benchmarks
if [ -n "$BLOCK_FILTER" ]; then
    for script in block-assembly-bench.sh block-serialization-bench.sh; do
        if [ -f "$BLLVM_BENCH_ROOT/scripts/core/$script" ]; then
            echo "✅ Updating $script"
            case "$script" in
                block-assembly-bench.sh)
                    BENCH_NAME=$(echo "$BLOCK_BENCHMARKS" | grep -i "assemble" | head -1 || echo "AssembleBlock")
                    sed -i "s|-filter=\"[^\"]*\"|-filter=\"$BENCH_NAME\"|g" "$BLLVM_BENCH_ROOT/scripts/core/$script"
                    ;;
                block-serialization-bench.sh)
                    # This one needs multiple benchmarks
                    BENCH_NAMES=$(echo "$BLOCK_BENCHMARKS" | grep -iE "read|write|deserialize" | tr '\n' '|' | sed 's/|$//' || echo "ReadBlock|WriteBlock|DeserializeBlock")
                    sed -i "s|-filter=\"[^\"]*\"|-filter=\"$BENCH_NAMES\"|g" "$BLLVM_BENCH_ROOT/scripts/core/$script"
                    ;;
            esac
        fi
    done
fi

# Update script-verification-bench.sh
if [ -n "$SCRIPT_FILTER" ] && [ -f "$BLLVM_BENCH_ROOT/scripts/core/script-verification-bench.sh" ]; then
    echo "✅ Updating script-verification-bench.sh"
    BENCH_NAMES=$(echo "$SCRIPT_BENCHMARKS" | tr '\n' '|' | sed 's/|$//')
    sed -i "s|-filter=\"[^\"]*\"|-filter=\"$BENCH_NAMES\"|g" "$BLLVM_BENCH_ROOT/scripts/core/script-verification-bench.sh"
    # Update grep pattern
    sed -i "s|grep -oE \"VerifyScriptBench|VerifyNestedIfScript\"|grep -oE \"$BENCH_NAMES\"|g" "$BLLVM_BENCH_ROOT/scripts/core/script-verification-bench.sh"
    sed -i "s|grep -qE \"VerifyScriptBench|VerifyNestedIfScript\"|grep -qE \"$BENCH_NAMES\"|g" "$BLLVM_BENCH_ROOT/scripts/core/script-verification-bench.sh"
fi

# Update hash benchmarks
if [ -n "$HASH_FILTER" ]; then
    for script in ripemd160-bench.sh merkle-tree-bench.sh; do
        if [ -f "$BLLVM_BENCH_ROOT/scripts/core/$script" ]; then
            echo "✅ Updating $script"
            case "$script" in
                ripemd160-bench.sh)
                    BENCH_NAME=$(echo "$HASH_BENCHMARKS" | grep -i "ripemd" | head -1 || echo "BenchRIPEMD160")
                    sed -i "s|-filter=\"[^\"]*\"|-filter=\"$BENCH_NAME\"|g" "$BLLVM_BENCH_ROOT/scripts/core/$script"
                    ;;
                merkle-tree-bench.sh)
                    BENCH_NAME=$(echo "$HASH_BENCHMARKS" | grep -i "merkle" | head -1 || echo "MerkleRoot")
                    sed -i "s|-filter=\"[^\"]*\"|-filter=\"$BENCH_NAME\"|g" "$BLLVM_BENCH_ROOT/scripts/core/$script"
                    ;;
            esac
        fi
    done
fi

# Update encoding benchmarks
if [ -n "$ENCODING_FILTER" ] && [ -f "$BLLVM_BENCH_ROOT/scripts/core/base58-bech32-bench.sh" ]; then
    echo "✅ Updating base58-bech32-bench.sh"
    BENCH_NAMES=$(echo "$ENCODING_BENCHMARKS" | tr '\n' '|' | sed 's/|$//')
    sed -i "s|-filter=\"[^\"]*\"|-filter=\"$BENCH_NAMES\"|g" "$BLLVM_BENCH_ROOT/scripts/core/base58-bech32-bench.sh"
fi

# Update UTXO benchmarks
if [ -n "$UTXO_FILTER" ] && [ -f "$BLLVM_BENCH_ROOT/scripts/core/utxo-caching-bench.sh" ]; then
    echo "✅ Updating utxo-caching-bench.sh"
    BENCH_NAMES=$(echo "$UTXO_BENCHMARKS" | tr '\n' '|' | sed 's/|$//')
    sed -i "s|-filter=\"[^\"]*\"|-filter=\"$BENCH_NAMES\"|g" "$BLLVM_BENCH_ROOT/scripts/core/utxo-caching-bench.sh"
fi

echo ""
echo "✅ All Core benchmark scripts updated!"
echo "   Review changes with: git diff scripts/core/"

