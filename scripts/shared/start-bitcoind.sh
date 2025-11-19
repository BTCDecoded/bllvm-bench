#!/bin/bash
# Helper function to start bitcoind in regtest mode
# Usage: start_bitcoind [rpc_port] [data_dir]

start_bitcoind() {
    local rpc_port="${1:-18443}"
    local data_dir="${2:-/tmp/bitcoin-regtest-$$}"
    local rpc_user="${3:-test}"
    local rpc_pass="${4:-test}"
    local rpc_host="${5:-127.0.0.1}"
    
    # Find bitcoind
    local bitcoind=""
    if [ -n "$CORE_PATH" ] && [ -d "$CORE_PATH" ]; then
        for path in "$CORE_PATH/build/bin/bitcoind" "$CORE_PATH/src/bitcoind" "$CORE_PATH/bin/bitcoind"; do
            if [ -f "$path" ] && [ -x "$path" ]; then
                bitcoind="$path"
                break
            fi
        done
    fi
    
    if [ -z "$bitcoind" ] && command -v bitcoind >/dev/null 2>&1; then
        bitcoind=$(command -v bitcoind)
    fi
    
    if [ -z "$bitcoind" ] || [ ! -f "$bitcoind" ]; then
        echo "bitcoind not found"
        return 1
    fi
    
    # Check if already running
    if curl -s --connect-timeout 1 -X POST \
        -u "$rpc_user:$rpc_pass" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","method":"getblockcount","params":[],"id":1}' \
        "http://$rpc_host:$rpc_port" > /dev/null 2>&1; then
        echo "bitcoind already running on port $rpc_port"
        return 0
    fi
    
    # Kill any existing bitcoind on this port
    pkill -f "bitcoind.*regtest.*$rpc_port" 2>/dev/null || true
    sleep 2
    
    # Create data directory
    mkdir -p "$data_dir"
    
    # Start bitcoind
    if ! "$bitcoind" -regtest -daemon -server \
        -datadir="$data_dir" \
        -rpcuser="$rpc_user" \
        -rpcpassword="$rpc_pass" \
        -rpcport="$rpc_port" \
        -rpcallowip=127.0.0.1 \
        -rpcbind=127.0.0.1 \
        -fallbackfee=0.00001 \
        -txindex=0 \
        > /tmp/bitcoind-startup-$$.log 2>&1; then
        echo "Failed to start bitcoind: $(cat /tmp/bitcoind-startup-$$.log 2>/dev/null | head -5)"
        return 1
    fi
    
    # Wait for it to be ready
    sleep 5
    for i in {1..60}; do
        if curl -s --connect-timeout 2 -X POST \
            -u "$rpc_user:$rpc_pass" \
            -H "Content-Type: application/json" \
            -d '{"jsonrpc":"2.0","method":"getblockcount","params":[],"id":1}' \
            "http://$rpc_host:$rpc_port" 2>/dev/null | jq -e '.result >= 0' > /dev/null 2>&1; then
            echo "bitcoind ready"
            return 0
        fi
        sleep 1
    done
    
    echo "bitcoind failed to start or connect after 60 seconds"
    return 1
}
