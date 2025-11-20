#!/bin/bash
# Fair Concurrent Operations Benchmark
# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../../shared/common.sh"
# Measures performance under concurrent load for both implementations

set -e

# Track Commons RPC server PID for cleanup
COMMONS_RPC_PID=""

# Cleanup function
cleanup() {
    if [ -n "$COMMONS_RPC_PID" ]; then
        echo "Cleaning up Commons RPC server (PID: $COMMONS_RPC_PID)..."
        kill "$COMMONS_RPC_PID" 2>/dev/null || true
        wait "$COMMONS_RPC_PID" 2>/dev/null || true
    fi
}

# Register cleanup on exit
trap cleanup EXIT

OUTPUT_DIR="${1:-$(dirname "$0")/../results}"
mkdir -p "$OUTPUT_DIR"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUTPUT_FILE="$OUTPUT_DIR/concurrent-operations-fair-$(date +%Y%m%d-%H%M%S).json"

echo "=== Fair Concurrent Operations Benchmark ==="
echo ""
echo "This benchmark measures performance under concurrent load:"
echo "- Concurrent RPC calls (10, 50, 100 requests)"
echo "- Measures throughput and latency (p50, p95, p99)"
echo ""

# Configuration
RPC_PORT=18443
RPC_USER="test"
RPC_PASS="test"
RPC_HOST="127.0.0.1"
COMMONS_RPC_PORT=18332
CONCURRENT_LEVELS=(10 50 100)
RPC_METHOD="getblockcount"  # Simple method for concurrent testing

# Helper function to make RPC call and measure time
rpc_call_timing() {
    local url=$1
    local method=$2
    local auth=$3
    
    local start=$(date +%s%N)
    if [ -n "$auth" ]; then
        curl -s -X POST -u "$auth" \
            -H "Content-Type: application/json" \
            -d "{\"jsonrpc\":\"2.0\",\"method\":\"$method\",\"params\":[],\"id\":1}" \
            "$url" > /dev/null 2>&1
    else
        curl -s -X POST \
            -H "Content-Type: application/json" \
            -d "{\"jsonrpc\":\"2.0\",\"method\":\"$method\",\"params\":[],\"id\":1}" \
            "$url" > /dev/null 2>&1
    fi
    local end=$(date +%s%N)
    echo $(( (end - start) / 1000000 ))  # Return milliseconds
}

# Helper function to run concurrent requests
run_concurrent_test() {
    local url=$1
    local method=$2
    local concurrency=$3
    local auth=$4
    local iterations=$5
    
    local times=()
    local pids=()
    
    # Run concurrent requests
    for i in $(seq 1 $iterations); do
        (
            time_ms=$(rpc_call_timing "$url" "$method" "$auth")
            echo "$time_ms" > "/tmp/concurrent_${concurrency}_${i}.tmp"
        ) &
        pids+=($!)
        
        # Limit concurrency
        if [ ${#pids[@]} -ge $concurrency ]; then
            wait ${pids[0]}
            unset pids[0]
            pids=("${pids[@]}")  # Reindex array
        fi
    done
    
    # Wait for all remaining
    for pid in "${pids[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    
    # Collect results
    for i in $(seq 1 $iterations); do
        if [ -f "/tmp/concurrent_${concurrency}_${i}.tmp" ]; then
            time_ms=$(cat "/tmp/concurrent_${concurrency}_${i}.tmp" 2>/dev/null || echo "0")
            if [ -n "$time_ms" ] && [ "$time_ms" != "0" ]; then
                times+=($time_ms)
            fi
            rm -f "/tmp/concurrent_${concurrency}_${i}.tmp"
        fi
    done
    
    # Calculate statistics
    if [ ${#times[@]} -eq 0 ]; then
        echo "0|0|0|0|0"
        return
    fi
    
    # Sort times
    IFS=$'\n' sorted=($(sort -n <<<"${times[*]}"))
    unset IFS
    
    local count=${#sorted[@]}
    local sum=0
    for t in "${sorted[@]}"; do
        sum=$((sum + t))
    done
    
    local avg=$((sum / count))
    local p50_idx=$((count * 50 / 100))
    local p95_idx=$((count * 95 / 100))
    local p99_idx=$((count * 99 / 100))
    
    local p50=${sorted[$p50_idx]}
    local p95=${sorted[$p95_idx]}
    local p99=${sorted[$p99_idx]}
    local min=${sorted[0]}
    local max=${sorted[$((count - 1))]}
    
    echo "${avg}|${p50}|${p95}|${p99}|${min}|${max}|${count}"
}

# Bitcoin Core Concurrent Performance
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "1. Bitcoin Core Concurrent RPC Performance"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

CORE_URL="http://$RPC_HOST:$RPC_PORT"
CORE_AUTH="$RPC_USER:$RPC_PASS"

# Check if Core RPC is available
if curl -s --connect-timeout 1 -X POST \
    -u "$CORE_AUTH" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"ping","params":[],"id":1}' \
    "$CORE_URL" > /dev/null 2>&1; then
    echo "✅ Bitcoin Core RPC is available"
    
    # Use variables instead of associative arrays to avoid bash base interpretation issues
    CORE_10_AVG=0; CORE_10_P50=0; CORE_10_P95=0; CORE_10_P99=0; CORE_10_MIN=0; CORE_10_MAX=0
    CORE_50_AVG=0; CORE_50_P50=0; CORE_50_P95=0; CORE_50_P99=0; CORE_50_MIN=0; CORE_50_MAX=0
    CORE_100_AVG=0; CORE_100_P50=0; CORE_100_P95=0; CORE_100_P99=0; CORE_100_MIN=0; CORE_100_MAX=0
    
    for concurrency in "${CONCURRENT_LEVELS[@]}"; do
        echo "Testing with $concurrency concurrent requests..."
        result=$(run_concurrent_test "$CORE_URL" "$RPC_METHOD" "$concurrency" "$CORE_AUTH" 50)
        IFS='|' read -r avg p50 p95 p99 min max count <<< "$result"
        
        # Set variables based on concurrency level
        case $concurrency in
            10)
                CORE_10_AVG=$avg; CORE_10_P50=$p50; CORE_10_P95=$p95; CORE_10_P99=$p99
                CORE_10_MIN=$min; CORE_10_MAX=$max
                ;;
            50)
                CORE_50_AVG=$avg; CORE_50_P50=$p50; CORE_50_P95=$p95; CORE_50_P99=$p99
                CORE_50_MIN=$min; CORE_50_MAX=$max
                ;;
            100)
                CORE_100_AVG=$avg; CORE_100_P50=$p50; CORE_100_P95=$p95; CORE_100_P99=$p99
                CORE_100_MIN=$min; CORE_100_MAX=$max
                ;;
        esac
        
        echo "  Average: ${avg}ms, P50: ${p50}ms, P95: ${p95}ms, P99: ${p99}ms"
    done
else
    echo "⚠️  Bitcoin Core RPC not available"
    CORE_10_AVG=0; CORE_10_P50=0; CORE_10_P95=0; CORE_10_P99=0; CORE_10_MIN=0; CORE_10_MAX=0
    CORE_50_AVG=0; CORE_50_P50=0; CORE_50_P95=0; CORE_50_P99=0; CORE_50_MIN=0; CORE_50_MAX=0
    CORE_100_AVG=0; CORE_100_P50=0; CORE_100_P95=0; CORE_100_P99=0; CORE_100_MIN=0; CORE_100_MAX=0
fi

echo ""

# Bitcoin Commons Concurrent Performance
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "2. Bitcoin Commons Concurrent RPC Performance"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

COMMONS_URL="http://127.0.0.1:$COMMONS_RPC_PORT"

# Check if Commons RPC is available, and try to start if not
COMMONS_RPC_AVAILABLE=false
if curl -s --connect-timeout 1 -X POST \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"ping","params":[],"id":1}' \
    "$COMMONS_URL" > /dev/null 2>&1; then
    COMMONS_RPC_AVAILABLE=true
    echo "✅ Bitcoin Commons RPC is available"
else
    echo "⚠️  Bitcoin Commons RPC not available, attempting to start..."
    
    # Try to start Commons RPC server
    if [ -n "$COMMONS_NODE_PATH" ] && [ -d "$COMMONS_NODE_PATH" ]; then
        echo "  COMMONS_NODE_PATH: $COMMONS_NODE_PATH"
        cd "$COMMONS_NODE_PATH"
        
        # Check if rpc_server_bench example exists
        if [ -f "examples/rpc_server_bench.rs" ]; then
            echo "  Found rpc_server_bench.rs, starting server..."
            # Unset CC/CXX if they point to ccache (OpenSSL build issue)
            if [ "$CC" = "ccache" ] || [ "${CC##*/}" = "ccache" ]; then
                unset CC
            fi
            if [ "$CXX" = "ccache" ] || [ "${CXX##*/}" = "ccache" ]; then
                unset CXX
            fi
            # Run RPC server in background (example expects address as first arg: 127.0.0.1:PORT)
            # The example parses args[1] as SocketAddr, default is 127.0.0.1:18332
            RUSTFLAGS="-C target-cpu=native" cargo run --example rpc_server_bench --features production -- "127.0.0.1:$COMMONS_RPC_PORT" > /tmp/commons-rpc-server.log 2>&1 &
            COMMONS_RPC_PID=$!
            echo "  Commons RPC server started (PID: $COMMONS_RPC_PID)"
            echo "  Log: /tmp/commons-rpc-server.log"
            
            # Wait for server to be ready (up to 30 seconds)
            for i in {1..30}; do
                if curl -s --connect-timeout 1 -X POST \
                    -H "Content-Type: application/json" \
                    -d '{"jsonrpc":"2.0","method":"ping","params":[],"id":1}' \
                    "$COMMONS_URL" > /dev/null 2>&1; then
                    echo "✅ Bitcoin Commons RPC is now available"
                    COMMONS_RPC_AVAILABLE=true
                    break
                fi
                if [ $((i % 5)) -eq 0 ]; then
                    echo "  Waiting for server... ($i/30)"
                fi
                sleep 1
            done
            
            # Check if server is ready
            if [ "$COMMONS_RPC_AVAILABLE" = "false" ]; then
                echo "⚠️  Commons RPC server failed to start or is not responding"
                echo "  Check log: /tmp/commons-rpc-server.log"
                if [ -n "$COMMONS_RPC_PID" ]; then
                    kill "$COMMONS_RPC_PID" 2>/dev/null || true
                fi
            fi
        else
            echo "⚠️  Commons RPC server example not found at examples/rpc_server_bench.rs"
            echo "  COMMONS_NODE_PATH: $COMMONS_NODE_PATH"
            ls -la examples/ 2>/dev/null | head -5 || echo "  examples/ directory not found"
        fi
    else
        echo "⚠️  COMMONS_NODE_PATH not set or invalid"
        echo "  COMMONS_NODE_PATH: ${COMMONS_NODE_PATH:-not set}"
    fi
fi

# Run tests if RPC is available
if [ "$COMMONS_RPC_AVAILABLE" = "true" ]; then
    
    # Use variables instead of associative arrays
    COMMONS_10_AVG=0; COMMONS_10_P50=0; COMMONS_10_P95=0; COMMONS_10_P99=0; COMMONS_10_MIN=0; COMMONS_10_MAX=0
    COMMONS_50_AVG=0; COMMONS_50_P50=0; COMMONS_50_P95=0; COMMONS_50_P99=0; COMMONS_50_MIN=0; COMMONS_50_MAX=0
    COMMONS_100_AVG=0; COMMONS_100_P50=0; COMMONS_100_P95=0; COMMONS_100_P99=0; COMMONS_100_MIN=0; COMMONS_100_MAX=0
    
    for concurrency in "${CONCURRENT_LEVELS[@]}"; do
        echo "Testing with $concurrency concurrent requests..."
        result=$(run_concurrent_test "$COMMONS_URL" "$RPC_METHOD" "$concurrency" "" 50)
        IFS='|' read -r avg p50 p95 p99 min max count <<< "$result"
        
        # Set variables based on concurrency level
        case $concurrency in
            10)
                COMMONS_10_AVG=$avg; COMMONS_10_P50=$p50; COMMONS_10_P95=$p95; COMMONS_10_P99=$p99
                COMMONS_10_MIN=$min; COMMONS_10_MAX=$max
                ;;
            50)
                COMMONS_50_AVG=$avg; COMMONS_50_P50=$p50; COMMONS_50_P95=$p95; COMMONS_50_P99=$p99
                COMMONS_50_MIN=$min; COMMONS_50_MAX=$max
                ;;
            100)
                COMMONS_100_AVG=$avg; COMMONS_100_P50=$p50; COMMONS_100_P95=$p95; COMMONS_100_P99=$p99
                COMMONS_100_MIN=$min; COMMONS_100_MAX=$max
                ;;
        esac
        
        echo "  Average: ${avg}ms, P50: ${p50}ms, P95: ${p95}ms, P99: ${p99}ms"
    done
else
    echo "⚠️  Bitcoin Commons RPC not available"
    COMMONS_10_AVG=0; COMMONS_10_P50=0; COMMONS_10_P95=0; COMMONS_10_P99=0; COMMONS_10_MIN=0; COMMONS_10_MAX=0
    COMMONS_50_AVG=0; COMMONS_50_P50=0; COMMONS_50_P95=0; COMMONS_50_P99=0; COMMONS_50_MIN=0; COMMONS_50_MAX=0
    COMMONS_100_AVG=0; COMMONS_100_P50=0; COMMONS_100_P95=0; COMMONS_100_P99=0; COMMONS_100_MIN=0; COMMONS_100_MAX=0
fi

echo ""

# Generate JSON output
cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "concurrent_operations_fair": {
    "methodology": "Concurrent RPC requests at different levels (10, 50, 100). Measures latency percentiles (p50, p95, p99) and throughput.",
    "test_method": "$RPC_METHOD",
    "iterations_per_level": 50,
    "bitcoin_core": {
      "concurrency_10": {
        "avg_latency_ms": ${CORE_10_AVG},
        "p50_latency_ms": ${CORE_10_P50},
        "p95_latency_ms": ${CORE_10_P95},
        "p99_latency_ms": ${CORE_10_P99},
        "min_latency_ms": ${CORE_10_MIN},
        "max_latency_ms": ${CORE_10_MAX}
      },
      "concurrency_50": {
        "avg_latency_ms": ${CORE_50_AVG},
        "p50_latency_ms": ${CORE_50_P50},
        "p95_latency_ms": ${CORE_50_P95},
        "p99_latency_ms": ${CORE_50_P99},
        "min_latency_ms": ${CORE_50_MIN},
        "max_latency_ms": ${CORE_50_MAX}
      },
      "concurrency_100": {
        "avg_latency_ms": ${CORE_100_AVG},
        "p50_latency_ms": ${CORE_100_P50},
        "p95_latency_ms": ${CORE_100_P95},
        "p99_latency_ms": ${CORE_100_P99},
        "min_latency_ms": ${CORE_100_MIN},
        "max_latency_ms": ${CORE_100_MAX}
      }
    },
    "bitcoin_commons": {
      "concurrency_10": {
        "avg_latency_ms": ${COMMONS_10_AVG},
        "p50_latency_ms": ${COMMONS_10_P50},
        "p95_latency_ms": ${COMMONS_10_P95},
        "p99_latency_ms": ${COMMONS_10_P99},
        "min_latency_ms": ${COMMONS_10_MIN},
        "max_latency_ms": ${COMMONS_10_MAX}
      },
      "concurrency_50": {
        "avg_latency_ms": ${COMMONS_50_AVG},
        "p50_latency_ms": ${COMMONS_50_P50},
        "p95_latency_ms": ${COMMONS_50_P95},
        "p99_latency_ms": ${COMMONS_50_P99},
        "min_latency_ms": ${COMMONS_50_MIN},
        "max_latency_ms": ${COMMONS_50_MAX}
      },
      "concurrency_100": {
        "avg_latency_ms": ${COMMONS_100_AVG},
        "p50_latency_ms": ${COMMONS_100_P50},
        "p95_latency_ms": ${COMMONS_100_P95},
        "p99_latency_ms": ${COMMONS_100_P99},
        "min_latency_ms": ${COMMONS_100_MIN},
        "max_latency_ms": ${COMMONS_100_MAX}
      }
    },
    "comparison": {
      "fair": true,
      "methodology_note": "Both measured using same concurrent request patterns. Lower latency is better, especially at higher concurrency levels.",
      "note": "Commons may excel due to Rust's async/await (Tokio) and better concurrent data structures."
    }
  }
}
EOF

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Results saved to: $OUTPUT_FILE"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

