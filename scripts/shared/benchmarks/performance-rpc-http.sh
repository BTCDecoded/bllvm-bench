#!/bin/bash
# RPC Performance Benchmark via HTTP (Fair Comparison)
# Measures RPC method execution times via HTTP/JSON-RPC to match Commons methodology
# This includes network overhead, HTTP parsing, and JSON-RPC overhead like Commons

set -e

# Set OUTPUT_FILE early so we can write error JSON even if sourcing fails
RESULTS_DIR_FALLBACK="${RESULTS_DIR:-$(pwd)/results}"
OUTPUT_DIR_FALLBACK="${RESULTS_DIR_FALLBACK}"
mkdir -p "$OUTPUT_DIR_FALLBACK" 2>/dev/null || true
OUTPUT_FILE="$OUTPUT_DIR_FALLBACK/performance-rpc-http-$(date +%Y%m%d-%H%M%S).json"

# Set trap to ensure JSON is always written, even on unexpected exit
trap 'if [ -n "$OUTPUT_FILE" ] && [ ! -f "$OUTPUT_FILE" ]; then echo "{\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"error\":\"Script exited unexpectedly before writing JSON\",\"script\":\"$0\"}" > "$OUTPUT_FILE" 2>/dev/null || true; fi' EXIT ERR

# Source common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../../shared/common.sh" || {
    echo "❌ Failed to source common.sh" >&2
    cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Failed to source common.sh",
  "script": "$0"
}
EOF
    exit 0
}

OUTPUT_DIR=$(get_output_dir "${1:-$RESULTS_DIR}")
OUTPUT_FILE="$OUTPUT_DIR/performance-rpc-http-$(date +%Y%m%d-%H%M%S).json"

# Find bitcoind and bitcoin-cli using CORE_PATH
BITCOIND=""
BITCOIN_CLI=""

if [ -n "$CORE_PATH" ] && [ -d "$CORE_PATH" ]; then
    # Try common locations
    for path in "$CORE_PATH/build/bin/bitcoind" "$CORE_PATH/src/bitcoind" "$CORE_PATH/bin/bitcoind"; do
        if [ -f "$path" ] && [ -x "$path" ]; then
            BITCOIND="$path"
            break
        fi
    done
    
    for path in "$CORE_PATH/build/bin/bitcoin-cli" "$CORE_PATH/src/bitcoin-cli" "$CORE_PATH/bin/bitcoin-cli"; do
        if [ -f "$path" ] && [ -x "$path" ]; then
            BITCOIN_CLI="$path"
            break
        fi
    done
fi

# Fallback: check PATH
if [ -z "$BITCOIND" ] && command -v bitcoind >/dev/null 2>&1; then
    BITCOIND=$(command -v bitcoind)
fi
if [ -z "$BITCOIN_CLI" ] && command -v bitcoin-cli >/dev/null 2>&1; then
    BITCOIN_CLI=$(command -v bitcoin-cli)
fi

echo "=== RPC Performance Benchmark via HTTP (Fair Comparison) ==="
echo ""
echo "⚠️  This measures Core via HTTP/JSON-RPC to match Commons methodology"
echo ""

# Check if bitcoind is running
RPC_PORT=18443
RPC_HOST="127.0.0.1"
RPC_USER="test"
RPC_PASS="test"

# Check if bitcoind is already running
BITCOIND_RUNNING=false
if curl -s --connect-timeout 1 -X POST \
    -u "$RPC_USER:$RPC_PASS" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"getblockcount","params":[],"id":1}' \
    "http://$RPC_HOST:$RPC_PORT" > /dev/null 2>&1; then
    echo "✅ bitcoind is already running"
    BITCOIND_RUNNING=true
fi

# If not running, try to start it
if [ "$BITCOIND_RUNNING" = false ]; then
    if [ -z "$BITCOIND" ] || [ ! -f "$BITCOIND" ]; then
        echo "❌ bitcoind not found"
        echo "   CORE_PATH: ${CORE_PATH:-not_set}"
        echo "   BITCOIND: ${BITCOIND:-not_found}"
        cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "bitcoind not found",
  "core_path": "${CORE_PATH:-not_set}",
  "bitcoind_path": "${BITCOIND:-not_found}",
  "note": "Please build Bitcoin Core with bitcoind binary, or ensure bitcoind is in PATH"
}
EOF
        echo "✅ Error JSON written to: $OUTPUT_FILE"
        exit 0
    fi
    
    echo "⚠️  bitcoind not running. Starting in regtest mode..."
    echo "   Using: $BITCOIND"
    
    # Kill any existing bitcoind on this port
    pkill -f "bitcoind.*regtest.*$RPC_PORT" 2>/dev/null || true
    sleep 2
    
    # Create data directory for regtest
    REGTEST_DIR="/tmp/bitcoin-regtest-$$"
    mkdir -p "$REGTEST_DIR"
    
    # Start bitcoind in regtest mode
    if ! "$BITCOIND" -regtest -daemon -server \
        -datadir="$REGTEST_DIR" \
        -rpcuser="$RPC_USER" \
        -rpcpassword="$RPC_PASS" \
        -rpcport="$RPC_PORT" \
        -rpcallowip=127.0.0.1 \
        -rpcbind=127.0.0.1 \
        -fallbackfee=0.00001 \
        -txindex=0 \
        > /tmp/bitcoind-startup.log 2>&1; then
        echo "❌ Failed to start bitcoind"
        echo "   Check: /tmp/bitcoind-startup.log"
        cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Failed to start bitcoind",
  "bitcoind_path": "$BITCOIND",
  "startup_log": "$(cat /tmp/bitcoind-startup.log 2>/dev/null | head -20 | jq -Rs . || echo 'null')",
  "note": "Check /tmp/bitcoind-startup.log for details"
}
EOF
        echo "✅ Error JSON written to: $OUTPUT_FILE"
        exit 0
    fi
    
    sleep 5
    
    # Wait for bitcoind to be ready
    BITCOIND_READY=false
    for i in {1..60}; do
        if curl -s --connect-timeout 2 -X POST \
            -u "$RPC_USER:$RPC_PASS" \
            -H "Content-Type: application/json" \
            -d '{"jsonrpc":"2.0","method":"getblockcount","params":[],"id":1}' \
            "http://$RPC_HOST:$RPC_PORT" 2>/dev/null | jq -e '.result >= 0' > /dev/null 2>&1; then
            echo "✅ bitcoind is ready (attempt $i/60)"
            BITCOIND_READY=true
            break
        fi
        if [ $i -lt 60 ]; then
            sleep 1
        fi
    done
    
    if [ "$BITCOIND_READY" = false ]; then
        echo "❌ Failed to start bitcoind or connect after 60 seconds"
        echo "   Checking bitcoind process..."
        ps aux | grep -E "bitcoind.*regtest" | grep -v grep || echo "   No bitcoind process found"
        echo "   Checking port $RPC_PORT..."
        netstat -tlnp 2>/dev/null | grep ":$RPC_PORT" || ss -tlnp 2>/dev/null | grep ":$RPC_PORT" || echo "   Port $RPC_PORT not in use"
        cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "error": "Failed to start bitcoind or connect",
  "bitcoind_path": "$BITCOIND",
  "rpc_port": $RPC_PORT,
  "startup_log": "$(cat /tmp/bitcoind-startup.log 2>/dev/null | head -50 | jq -Rs . || echo 'null')",
  "note": "bitcoind may have failed to start or is not responding. Check logs and ensure port $RPC_PORT is available."
}
EOF
        echo "✅ Error JSON written to: $OUTPUT_FILE"
        exit 0
    fi
fi

RPC_METHODS=(
    # Basic blockchain info
    "getblockchaininfo"
    "getblockcount"
    "getbestblockhash"
    "getblockhash"
    "getblock"
    "getblockheader"
    # Network info
    "getnetworkinfo"
    "getconnectioncount"
    "getnettotals"
    "getpeerinfo"
    # Mempool
    "getmempoolinfo"
    "getrawmempool"
    "getmempoolentry"
    # Chain state
    "gettxoutsetinfo"
    "getchaintips"
    "getchaintxstats"
    # Mining
    "getdifficulty"
    "getmininginfo"
    "getnetworkhashps"
    # Utility
    "ping"
    "uptime"
    "getmemoryinfo"
    "getrpcinfo"
    # Wallet (if available)
    "getwalletinfo"
    "listwallets"
)

cat > "$OUTPUT_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "measurement_method": "HTTP/JSON-RPC (fair comparison with Commons)",
  "rpc_server": "http://$RPC_HOST:$RPC_PORT",
  "rpc_performance": {
EOF

FIRST=true
for method in "${RPC_METHODS[@]}"; do
    echo "  Testing: $method (via HTTP)..."
    
    # Determine parameters based on method
    PARAMS="[]"
    case "$method" in
        "getblockhash")
            # Get block hash for block 0 (genesis)
            PARAMS="[0]"
            ;;
        "getblock")
            # Get block 0 (genesis) - verbosity 1 (JSON)
            GENESIS_HASH=$(curl -s -X POST -u "$RPC_USER:$RPC_PASS" -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"getblockhash","params":[0],"id":1}' "http://$RPC_HOST:$RPC_PORT" | jq -r '.result' 2>/dev/null || echo "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206")
            PARAMS="[\"$GENESIS_HASH\", 1]"
            ;;
        "getblockheader")
            # Get block header for block 0
            GENESIS_HASH=$(curl -s -X POST -u "$RPC_USER:$RPC_PASS" -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"getblockhash","params":[0],"id":1}' "http://$RPC_HOST:$RPC_PORT" | jq -r '.result' 2>/dev/null || echo "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206")
            PARAMS="[\"$GENESIS_HASH\"]"
            ;;
        "getmempoolentry")
            # Skip if mempool is empty, otherwise get first txid
            TXID=$(curl -s -X POST -u "$RPC_USER:$RPC_PASS" -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"getrawmempool","params":[],"id":1}' "http://$RPC_HOST:$RPC_PORT" | jq -r '.result[0]' 2>/dev/null || echo "")
            if [ -z "$TXID" ] || [ "$TXID" = "null" ]; then
                echo "    ⏭️  Skipping (mempool empty)"
                continue
            fi
            PARAMS="[\"$TXID\"]"
            ;;
        "getchaintxstats")
            # Get chain tx stats with no parameters (uses default)
            PARAMS="[]"
            ;;
    esac
    
    # Run multiple times and take average
    TIMES=()
    SKIP_METHOD=false
    for i in {1..20}; do
        START=$(date +%s%N)
        RESPONSE=$(curl -s -X POST \
            -u "$RPC_USER:$RPC_PASS" \
            -H "Content-Type: application/json" \
            -d "{\"jsonrpc\":\"2.0\",\"method\":\"$method\",\"params\":$PARAMS,\"id\":1}" \
            "http://$RPC_HOST:$RPC_PORT" 2>&1)
        END=$(date +%s%N)
        DURATION=$(( (END - START) / 1000000 ))
        
        # Check if method failed (not available or error)
        if echo "$RESPONSE" | jq -e '.error' > /dev/null 2>&1; then
            ERROR_CODE=$(echo "$RESPONSE" | jq -r '.error.code' 2>/dev/null || echo "")
            if [ "$ERROR_CODE" = "-32601" ] || [ "$ERROR_CODE" = "-1" ]; then
                echo "    ⏭️  Skipping (method not available or requires different parameters)"
                SKIP_METHOD=true
                break
            fi
        fi
        
        TIMES+=($DURATION)
    done
    
    if [ "$SKIP_METHOD" = true ]; then
        continue
    fi
    
    if [ "$FIRST" = false ]; then
        echo "," >> "$OUTPUT_FILE"
    fi
    FIRST=false
    
    # Sort times for percentile calculation
    IFS=$'\n' SORTED_TIMES=($(sort -n <<<"${TIMES[*]}"))
    unset IFS
    
    # Calculate average
    TOTAL=0
    for t in "${TIMES[@]}"; do
        TOTAL=$((TOTAL + t))
    done
    AVG=$(awk "BEGIN {printf \"%.2f\", $TOTAL / ${#TIMES[@]}}")
    
    # Calculate min and max
    MIN=${SORTED_TIMES[0]}
    MAX=${SORTED_TIMES[-1]}
    
    # Calculate percentiles (50th = median, 90th, 95th)
    PERCENTILE_50_INDEX=$(( ${#SORTED_TIMES[@]} * 50 / 100 ))
    PERCENTILE_90_INDEX=$(( ${#SORTED_TIMES[@]} * 90 / 100 ))
    PERCENTILE_95_INDEX=$(( ${#SORTED_TIMES[@]} * 95 / 100 ))
    
    PERCENTILE_50=${SORTED_TIMES[$PERCENTILE_50_INDEX]}
    PERCENTILE_90=${SORTED_TIMES[$PERCENTILE_90_INDEX]}
    PERCENTILE_95=${SORTED_TIMES[$PERCENTILE_95_INDEX]}
    
    cat >> "$OUTPUT_FILE" << EOF
    "$method": {
      "average_ms": $AVG,
      "min_ms": $MIN,
      "max_ms": $MAX,
      "median_ms": $PERCENTILE_50,
      "p90_ms": $PERCENTILE_90,
      "p95_ms": $PERCENTILE_95,
      "samples": ${#TIMES[@]}
    }
EOF
done

cat >> "$OUTPUT_FILE" << EOF
  },
  "comparison_note": "This benchmark measures Core via HTTP/JSON-RPC to match Commons methodology. Includes network overhead, HTTP parsing, and JSON-RPC overhead like Commons measurements."
}
EOF

echo ""
echo "Results saved to: $OUTPUT_FILE"
echo ""
echo "✅ Core RPC performance measured via HTTP (fair comparison)"
cat "$OUTPUT_FILE" | jq '.' 2>/dev/null || cat "$OUTPUT_FILE"

