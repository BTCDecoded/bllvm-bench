#!/bin/bash
# Deep Commons Analysis - Low-Level Performance Metrics
# Similar to bench_bitcoin's deep Core analysis, but for Commons
# Extracts CPU cycles, instructions, cache performance, etc.

set -e

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../shared/common.sh"

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
OUTPUT_FILE="$OUTPUT_DIR/commons-deep-analysis-$TIMESTAMP.json"

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Deep Commons Analysis - Low-Level Performance Metrics       ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""

BENCH_DIR="$BLLVM_BENCH_ROOT"

if [ ! -d "$BENCH_DIR" ]; then
    echo "❌ bllvm-bench directory not found"
    exit 1
fi

cd "$BENCH_DIR"

# Check if perf is available
if ! command -v perf >/dev/null 2>&1; then
    echo "⚠️  perf not available - CPU metrics will be limited"
    echo "   Install with: sudo apt-get install linux-perf (or equivalent)"
    USE_PERF=false
else
    USE_PERF=true
    echo "✅ perf available - will collect CPU metrics"
fi

echo ""
echo "Running deep analysis benchmarks..."
echo "This will collect:"
echo "  - CPU cycles"
echo "  - Instructions"
echo "  - Cache performance (L1/L2/L3 misses)"
echo "  - Branch prediction misses"
echo "  - Memory bandwidth"
echo ""

LOG_FILE="/tmp/commons-deep-analysis.log"
BENCHMARKS="[]"

# Run benchmarks with perf if available
if [ "$USE_PERF" = "true" ]; then
    echo "Running with perf instrumentation..."
    echo "Note: This will run benchmarks multiple times - once for each metric collection"
    echo ""
    
    # Run a representative set of benchmarks with perf
    for bench_name in "hash_operations" "block_validation_realistic" "mempool_operations"; do
        echo "  Benchmarking: $bench_name"
        
        # Run perf stat on the benchmark
        # Note: We need to run the actual benchmark binary, not cargo bench
        # For now, we'll use perf stat on cargo bench and parse the output
        perf stat -e cycles,instructions,cache-references,cache-misses,branch-instructions,branch-misses,LLC-loads,LLC-load-misses \
            -x, -o "/tmp/perf-$bench_name.csv" \
            cargo bench --bench "$bench_name" --features production --quiet 2>&1 | tee -a "$LOG_FILE" || true
        
        # Parse perf CSV output
        if [ -f "/tmp/perf-$bench_name.csv" ]; then
            # Parse CSV format: value,unit,event_name
            CYCLES=$(grep -E "^[0-9]+,cycles" /tmp/perf-$bench_name.csv | cut -d',' -f1 | head -1 || echo "")
            INSTRUCTIONS=$(grep -E "^[0-9]+,instructions" /tmp/perf-$bench_name.csv | cut -d',' -f1 | head -1 || echo "")
            CACHE_REF=$(grep -E "^[0-9]+,cache-references" /tmp/perf-$bench_name.csv | cut -d',' -f1 | head -1 || echo "")
            CACHE_MISS=$(grep -E "^[0-9]+,cache-misses" /tmp/perf-$bench_name.csv | cut -d',' -f1 | head -1 || echo "")
            BRANCH_INST=$(grep -E "^[0-9]+,branch-instructions" /tmp/perf-$bench_name.csv | cut -d',' -f1 | head -1 || echo "")
            BRANCH_MISS=$(grep -E "^[0-9]+,branch-misses" /tmp/perf-$bench_name.csv | cut -d',' -f1 | head -1 || echo "")
            LLC_LOADS=$(grep -E "^[0-9]+,LLC-loads" /tmp/perf-$bench_name.csv | cut -d',' -f1 | head -1 || echo "")
            LLC_MISS=$(grep -E "^[0-9]+,LLC-load-misses" /tmp/perf-$bench_name.csv | cut -d',' -f1 | head -1 || echo "")
            
            # Calculate metrics
            IPC="0"
            if [ -n "$CYCLES" ] && [ -n "$INSTRUCTIONS" ] && [ "$INSTRUCTIONS" != "0" ] && [ "$CYCLES" != "0" ]; then
                IPC=$(awk "BEGIN {printf \"%.4f\", $INSTRUCTIONS / $CYCLES}" 2>/dev/null || echo "0")
            fi
            
            CACHE_MISS_RATE="0"
            if [ -n "$CACHE_REF" ] && [ "$CACHE_REF" != "0" ]; then
                CACHE_MISS_RATE=$(awk "BEGIN {printf \"%.4f\", $CACHE_MISS / $CACHE_REF * 100}" 2>/dev/null || echo "0")
            fi
            
            BRANCH_MISS_RATE="0"
            if [ -n "$BRANCH_INST" ] && [ "$BRANCH_INST" != "0" ]; then
                BRANCH_MISS_RATE=$(awk "BEGIN {printf \"%.4f\", $BRANCH_MISS / $BRANCH_INST * 100}" 2>/dev/null || echo "0")
            fi
            
            # Convert to numbers (handle scientific notation)
            CYCLES_NUM=$(echo "$CYCLES" | sed 's/[^0-9.]//g' | head -1 || echo "0")
            INSTRUCTIONS_NUM=$(echo "$INSTRUCTIONS" | sed 's/[^0-9.]//g' | head -1 || echo "0")
            CACHE_REF_NUM=$(echo "$CACHE_REF" | sed 's/[^0-9.]//g' | head -1 || echo "0")
            CACHE_MISS_NUM=$(echo "$CACHE_MISS" | sed 's/[^0-9.]//g' | head -1 || echo "0")
            BRANCH_INST_NUM=$(echo "$BRANCH_INST" | sed 's/[^0-9.]//g' | head -1 || echo "0")
            BRANCH_MISS_NUM=$(echo "$BRANCH_MISS" | sed 's/[^0-9.]//g' | head -1 || echo "0")
            LLC_LOADS_NUM=$(echo "$LLC_LOADS" | sed 's/[^0-9.]//g' | head -1 || echo "0")
            LLC_MISS_NUM=$(echo "$LLC_MISS" | sed 's/[^0-9.]//g' | head -1 || echo "0")
            
            # Use direct number substitution (no --argjson needed)
            BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$bench_name" \
                ". += [{
                    \"name\": \$name,
                    \"cpu_metrics\": {
                        \"cycles\": $CYCLES_NUM,
                        \"instructions\": $INSTRUCTIONS_NUM,
                        "ipc": ($ipc | tonumber),
                        "cache": {
                            "references": ($cache_ref | tonumber),
                            "misses": ($cache_miss | tonumber),
                            "miss_rate_percent": ($cache_miss_rate | tonumber)
                        },
                        \"branch\": {
                            \"instructions\": $BRANCH_INST_NUM,
                            \"misses\": $BRANCH_MISS_NUM,
                            \"miss_rate_percent\": $BRANCH_MISS_RATE
                        },
                        \"llc\": {
                            \"loads\": $LLC_LOADS_NUM,
                            \"misses\": $LLC_MISS_NUM
                        }
                    }
                }]" 2>/dev/null || echo "$BENCHMARKS")
        fi
    done
else
    echo "Running without perf - collecting basic metrics only..."
    cargo bench --features production 2>&1 | tee "$LOG_FILE" || true
fi

# Also extract Criterion statistical data
CRITERION_DIR="$BENCH_DIR/target/criterion"
for bench_dir in "$CRITERION_DIR"/*; do
    if [ -d "$bench_dir" ] && [ -f "$bench_dir/base/estimates.json" ]; then
        BENCH_NAME=$(basename "$bench_dir")
        STATS=$("$BLLVM_BENCH_ROOT/scripts/shared/extract-criterion-stats.sh" "$bench_dir/base/estimates.json")
        
        # Add to benchmarks if not already present
        if ! echo "$BENCHMARKS" | jq -e ".[] | select(.name == \"$BENCH_NAME\")" >/dev/null 2>&1; then
            TIME_NS=$(jq -r '.mean.point_estimate' "$bench_dir/base/estimates.json" 2>/dev/null || echo "")
            if [ -n "$TIME_NS" ] && [ "$TIME_NS" != "null" ] && [ "$TIME_NS" != "0" ]; then
                BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$BENCH_NAME" \
                    --slurpfile stats "$STATS" \
                    '. += [{
                        "name": $name,
                        \"statistics\": \$stats[0][0]
                    }]' 2>/dev/null || echo "$BENCHMARKS")
            fi
        fi
    fi
done

# Get system info
CPU_INFO=$(lscpu 2>/dev/null | grep -E "Model name|CPU\(s\)|Thread|Core|Socket" | head -5 || echo "N/A")
MEM_INFO=$(free -h 2>/dev/null | grep "Mem:" || echo "N/A")

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "analysis_type": "deep_commons_analysis",
  "measurement_method": "Criterion benchmarks with perf instrumentation",
  "system_info": {
    "cpu": $(echo "$CPU_INFO" | jq -Rs .),
    "memory": $(echo "$MEM_INFO" | jq -Rs .),
    "perf_available": $USE_PERF
  },
  "benchmarks": $BENCHMARKS,
  "log_file": "$LOG_FILE",
  "note": "Deep analysis for Commons performance optimization and understanding"
}
EOF

echo ""
echo "✅ Deep analysis complete: $OUTPUT_FILE"
echo ""
echo "Metrics collected:"
echo "  - CPU cycles: $(echo "$BENCHMARKS" | jq '[.[] | select(.cpu_metrics) | .cpu_metrics.cycles] | add // 0' 2>/dev/null || echo "N/A")"
echo "  - Instructions: $(echo "$BENCHMARKS" | jq '[.[] | select(.cpu_metrics) | .cpu_metrics.instructions] | add // 0' 2>/dev/null || echo "N/A")"
echo "  - Benchmarks analyzed: $(echo "$BENCHMARKS" | jq 'length' 2>/dev/null || echo "0")"

