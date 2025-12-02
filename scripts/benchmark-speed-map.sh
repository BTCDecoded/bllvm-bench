#!/bin/bash
# Benchmark Speed Categorization Map
# Maps benchmark script names to speed categories: fast, medium, slow

# Usage: source this file and use get_benchmark_speed <benchmark_name>

declare -A SPEED_MAP

# Fast Benchmarks (< 2 minutes each)
# Quick validation and core operations
SPEED_MAP["block-validation-bench"]="fast"
SPEED_MAP["transaction-validation-bench"]="fast"
SPEED_MAP["mempool-operations-bench"]="fast"
SPEED_MAP["ripemd160-bench"]="fast"
SPEED_MAP["base58-bech32-bench"]="fast"
SPEED_MAP["duplicate-inputs-bench"]="fast"
SPEED_MAP["transaction-id-bench"]="fast"
SPEED_MAP["hash-micro-bench"]="fast"
SPEED_MAP["merkle-root-bench"]="fast"
SPEED_MAP["standard-tx-bench"]="fast"
SPEED_MAP["transaction-sighash-bench"]="fast"
SPEED_MAP["script-verification-bench"]="fast"
SPEED_MAP["utxo-caching-bench"]="fast"
SPEED_MAP["mempool-acceptance-bench"]="fast"
SPEED_MAP["mempool-bench"]="fast"

# Medium Benchmarks (2-10 minutes each)
# Moderate complexity operations
SPEED_MAP["block-serialization-bench"]="medium"
SPEED_MAP["transaction-serialization-bench"]="medium"
SPEED_MAP["compact-block-encoding-bench"]="medium"
SPEED_MAP["mempool-rbf-bench"]="medium"
SPEED_MAP["segwit-bench"]="medium"
SPEED_MAP["performance-rpc-http"]="medium"
SPEED_MAP["memory-efficiency-fair"]="medium"
SPEED_MAP["concurrent-operations-fair"]="medium"
SPEED_MAP["block-assembly-bench"]="medium"
SPEED_MAP["connectblock-bench"]="medium"
SPEED_MAP["merkle-tree-bench"]="medium"
SPEED_MAP["merkle-tree-precomputed-bench"]="medium"
SPEED_MAP["parallel-block-validation-bench"]="medium"

# Slow Benchmarks (> 10 minutes each)
# Complex, deep analysis, full sync
SPEED_MAP["deep-analysis-bench"]="slow"
SPEED_MAP["node-sync-rpc-bench"]="slow"

# Function to get speed category for a benchmark
# Handles both "core-*" and "commons-*" prefixes
get_benchmark_speed() {
    local bench_name="$1"
    
    # Remove prefixes (core-, commons-)
    local base_name="${bench_name#core-}"
    base_name="${base_name#commons-}"
    
    # Look up in map
    local speed="${SPEED_MAP[$base_name]}"
    
    # Default to medium if not found
    echo "${speed:-medium}"
}

# Function to get all benchmarks for a speed category
get_benchmarks_by_speed() {
    local speed="$1"
    local benchmarks=()
    
    for bench in "${!SPEED_MAP[@]}"; do
        if [ "${SPEED_MAP[$bench]}" = "$speed" ]; then
            benchmarks+=("$bench")
        fi
    done
    
    printf '%s\n' "${benchmarks[@]}" | sort
}

# Export functions if sourced
if [ "${BASH_SOURCE[0]}" != "${0}" ]; then
    export -f get_benchmark_speed
    export -f get_benchmarks_by_speed
fi

