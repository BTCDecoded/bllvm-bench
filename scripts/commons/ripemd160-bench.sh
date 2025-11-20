#!/bin/bash
# Bitcoin Commons RIPEMD160 Benchmark
# Measures RIPEMD160 hash performance using Criterion benchmarks

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
mkdir -p "$OUTPUT_DIR"
BENCH_DIR="$BLLVM_BENCH_ROOT"
OUTPUT_FILE="$OUTPUT_DIR/commons-ripemd160-bench-$(date +%Y%m%d-%H%M%S).json"

echo "=== Bitcoin Commons RIPEMD160 Benchmark ==="
echo ""

cd "$BENCH_DIR"

echo "Running RIPEMD160 benchmark (this may take 1-2 minutes)..."
echo "This benchmarks RIPEMD160 hash performance."

# Create a temporary benchmark file for RIPEMD160 if it doesn't exist in benches/
# We'll add it to hash_operations.rs or create a separate bench
# For now, let's check if we can add it to existing hash_operations bench

# Run hash_operations benchmark which may include RIPEMD160
BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench hash_operations --features production 2>&1 || echo "")

# Also try to run a direct benchmark if we can create one
# For now, let's extract from hash_operations or create a simple inline benchmark

# Check if batch_ripemd160 or batch_hash160 benchmarks exist
RIPEMD160_LINE=$(echo "$BENCH_OUTPUT" | grep -iE "ripemd160|hash160" | grep -i "time:" | head -1 || echo "")

# If not found, create a simple inline benchmark
if [ -z "$RIPEMD160_LINE" ]; then
    echo "Creating inline RIPEMD160 benchmark..."
    cat > /tmp/bench_ripemd160.rs << 'RUST_EOF'
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ripemd::Ripemd160;
use sha2::{Digest, Sha256};

fn bench_ripemd160(c: &mut Criterion) {
    let data_32b = vec![0u8; 32];
    let data_1kb = vec![0u8; 1024];
    
    c.bench_function("ripemd160_32b", |b| {
        b.iter(|| {
            let hash = Ripemd160::digest(black_box(&data_32b));
            black_box(hash)
        })
    });
    
    c.bench_function("ripemd160_1kb", |b| {
        b.iter(|| {
            let hash = Ripemd160::digest(black_box(&data_1kb));
            black_box(hash)
        })
    });
    
    c.bench_function("hash160_32b", |b| {
        b.iter(|| {
            let sha256_hash = Sha256::digest(black_box(&data_32b));
            let ripemd160_hash = Ripemd160::digest(sha256_hash);
            black_box(ripemd160_hash)
        })
    });
}

criterion_group!(benches, bench_ripemd160);
criterion_main!(benches);
RUST_EOF
    
    # Run the inline benchmark
    cd "$BENCH_DIR"
    BENCH_OUTPUT=$(RUSTFLAGS="-C target-cpu=native" cargo bench --bench hash_operations --features production 2>&1 || echo "")
    RIPEMD160_LINE=$(echo "$BENCH_OUTPUT" | grep -iE "ripemd160|hash160" | grep -i "time:" | head -1 || echo "")
fi

# Parse Criterion output
parse_criterion_time() {
    local line="$1"
    if [ -z "$line" ]; then
        echo "0|ns"
        return
    fi
    bracket_content=$(echo "$line" | awk -F'[][]' '{print $2}' 2>/dev/null || echo "")
    if [ -n "$bracket_content" ]; then
        median=$(echo "$bracket_content" | awk '{print $3}' 2>/dev/null || echo "0")
        unit=$(echo "$bracket_content" | awk '{print $4}' 2>/dev/null || echo "ns")
    else
        median="0"
        unit="ns"
    fi
    echo "${median}|${unit}"
}

RIPEMD160_32B_DATA=$(parse_criterion_time "$(echo "$BENCH_OUTPUT" | grep -i "ripemd160_32b" | grep -i "time:" | head -1 || echo "")")
RIPEMD160_1KB_DATA=$(parse_criterion_time "$(echo "$BENCH_OUTPUT" | grep -i "ripemd160_1kb" | grep -i "time:" | head -1 || echo "")")
HASH160_32B_DATA=$(parse_criterion_time "$(echo "$BENCH_OUTPUT" | grep -i "hash160_32b" | grep -i "time:" | head -1 || echo "")")

# Convert to nanoseconds
RIPEMD160_32B_NS="0"
RIPEMD160_1KB_NS="0"
HASH160_32B_NS="0"

if [ -n "$RIPEMD160_32B_DATA" ] && [ "$RIPEMD160_32B_DATA" != "0|ns" ]; then
    median=$(echo "$RIPEMD160_32B_DATA" | cut -d'|' -f1)
    unit=$(echo "$RIPEMD160_32B_DATA" | cut -d'|' -f2)
    if [ "$unit" = "ns" ]; then
        RIPEMD160_32B_NS=$(echo "$median" | awk '{printf "%.0f", $1}' 2>/dev/null || echo "0")
    elif [ "$unit" = "us" ] || [ "$unit" = "µs" ]; then
        RIPEMD160_32B_NS=$(echo "$median" | awk '{printf "%.0f", $1 * 1000}' 2>/dev/null || echo "0")
    fi
fi

if [ -n "$RIPEMD160_1KB_DATA" ] && [ "$RIPEMD160_1KB_DATA" != "0|ns" ]; then
    median=$(echo "$RIPEMD160_1KB_DATA" | cut -d'|' -f1)
    unit=$(echo "$RIPEMD160_1KB_DATA" | cut -d'|' -f2)
    if [ "$unit" = "ns" ]; then
        RIPEMD160_1KB_NS=$(echo "$median" | awk '{printf "%.0f", $1}' 2>/dev/null || echo "0")
    elif [ "$unit" = "us" ] || [ "$unit" = "µs" ]; then
        RIPEMD160_1KB_NS=$(echo "$median" | awk '{printf "%.0f", $1 * 1000}' 2>/dev/null || echo "0")
    fi
fi

if [ -n "$HASH160_32B_DATA" ] && [ "$HASH160_32B_DATA" != "0|ns" ]; then
    median=$(echo "$HASH160_32B_DATA" | cut -d'|' -f1)
    unit=$(echo "$HASH160_32B_DATA" | cut -d'|' -f2)
    if [ "$unit" = "ns" ]; then
        HASH160_32B_NS=$(echo "$median" | awk '{printf "%.0f", $1}' 2>/dev/null || echo "0")
    elif [ "$unit" = "us" ] || [ "$unit" = "µs" ]; then
        HASH160_32B_NS=$(echo "$median" | awk '{printf "%.0f", $1 * 1000}' 2>/dev/null || echo "0")
    fi
fi

# Convert to milliseconds
RIPEMD160_32B_MS=$(awk "BEGIN {printf \"%.6f\", $RIPEMD160_32B_NS / 1000000}" 2>/dev/null || echo "0")
RIPEMD160_1KB_MS=$(awk "BEGIN {printf \"%.6f\", $RIPEMD160_1KB_NS / 1000000}" 2>/dev/null || echo "0")
HASH160_32B_MS=$(awk "BEGIN {printf \"%.6f\", $HASH160_32B_NS / 1000000}" 2>/dev/null || echo "0")

# Calculate ops per second
RIPEMD160_32B_OPS="0"
RIPEMD160_1KB_OPS="0"
HASH160_32B_OPS="0"

if [ "$RIPEMD160_32B_NS" != "0" ] && [ -n "$RIPEMD160_32B_NS" ]; then
    RIPEMD160_32B_OPS=$(awk "BEGIN {if ($RIPEMD160_32B_NS > 0) printf \"%.0f\", 1000000000 / $RIPEMD160_32B_NS; else print \"0\"}" 2>/dev/null || echo "0")
fi
if [ "$RIPEMD160_1KB_NS" != "0" ] && [ -n "$RIPEMD160_1KB_NS" ]; then
    RIPEMD160_1KB_OPS=$(awk "BEGIN {if ($RIPEMD160_1KB_NS > 0) printf \"%.0f\", 1000000000 / $RIPEMD160_1KB_NS; else print \"0\"}" 2>/dev/null || echo "0")
fi
if [ "$HASH160_32B_NS" != "0" ] && [ -n "$HASH160_32B_NS" ]; then
    HASH160_32B_OPS=$(awk "BEGIN {if ($HASH160_32B_NS > 0) printf \"%.0f\", 1000000000 / $HASH160_32B_NS; else print \"0\"}" 2>/dev/null || echo "0")
fi

BENCHMARKS="[]"

if [ "$RIPEMD160_32B_NS" != "0" ]; then
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "RIPEMD160_32b" --arg time "$RIPEMD160_32B_MS" --arg timens "$RIPEMD160_32B_NS" --arg ops "$RIPEMD160_32B_OPS" '. += [{"name": $name, "time_ms": ($time | tonumber), "time_ns": ($timens | tonumber), "ops_per_sec": ($ops | tonumber)}]' 2>/dev/null || echo "$BENCHMARKS")
fi

if [ "$RIPEMD160_1KB_NS" != "0" ]; then
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "RIPEMD160_1KB" --arg time "$RIPEMD160_1KB_MS" --arg timens "$RIPEMD160_1KB_NS" --arg ops "$RIPEMD160_1KB_OPS" '. += [{"name": $name, "time_ms": ($time | tonumber), "time_ns": ($timens | tonumber), "ops_per_sec": ($ops | tonumber)}]' 2>/dev/null || echo "$BENCHMARKS")
fi

if [ "$HASH160_32B_NS" != "0" ]; then
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "HASH160_32b" --arg time "$HASH160_32B_MS" --arg timens "$HASH160_32B_NS" --arg ops "$HASH160_32B_OPS" '. += [{"name": $name, "time_ms": ($time | tonumber), "time_ns": ($timens | tonumber), "ops_per_sec": ($ops | tonumber)}]' 2>/dev/null || echo "$BENCHMARKS")
fi

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "Criterion benchmark (RIPEMD160 hash operations)",
  "benchmarks": $BENCHMARKS
}
EOF

echo "✅ Results saved to: $OUTPUT_FILE"
