# Reusing Existing Bitcoin Core Nodes

The differential testing system can now automatically detect and reuse existing Bitcoin Core nodes running on your system, whether they're on regtest, signet, testnet, or mainnet.

## How It Works

### Automatic Discovery

The system automatically scans common Bitcoin Core RPC ports:
- **Mainnet**: 8332
- **Testnet**: 18332
- **Regtest**: 18443
- **Signet**: 38332

It detects what network each node is on by calling `getblockchaininfo` RPC and checking the `chain` field.

### Network Preference

You can specify which network to use via environment variable:

```bash
export BITCOIN_NETWORK=regtest  # or mainnet, testnet, signet
```

If a node on the preferred network is found, it will be used. Otherwise, it will use the first available node, or start a new regtest node if none are found.

### RPC Credentials

Configure RPC credentials via environment variables:

```bash
export BITCOIN_RPC_USER=your_username
export BITCOIN_RPC_PASSWORD=your_password
export BITCOIN_RPC_HOST=127.0.0.1  # default
export BITCOIN_RPC_PORT=18443      # custom port (optional)
```

**Defaults**: `test` / `test` (for regtest nodes)

## Usage Examples

### Use Existing Regtest Node

```bash
# Start your own regtest node
bitcoind -regtest -daemon -rpcuser=test -rpcpassword=test -rpcport=18443

# Run tests - will automatically detect and use it
export BITCOIN_NETWORK=regtest
cargo test --features differential --test integration
```

### Use Existing Signet Node

```bash
# Your signet node is already running on port 38332

# Run tests - will detect and use signet
export BITCOIN_NETWORK=signet
export BITCOIN_RPC_USER=your_signet_user
export BITCOIN_RPC_PASSWORD=your_signet_pass
cargo test --features differential --test integration
```

### Use Any Available Node

```bash
# Don't specify network - will use first available
cargo test --features differential --test integration
```

## Behavior

1. **Discovery**: Scans common ports for running nodes
2. **Network Detection**: Queries each node to determine network type
3. **Reuse**: Uses existing node if found (much faster - no startup time)
4. **Fallback**: Starts new regtest node if none found
5. **Cleanup**: Only stops nodes it started (won't touch reused nodes)

## Benefits

- ✅ **Faster tests**: No startup time when reusing existing nodes
- ✅ **CI-friendly**: Works with pre-configured nodes on runners
- ✅ **Flexible**: Supports all Bitcoin networks
- ✅ **Safe**: Won't interfere with your production nodes
- ✅ **Configurable**: Environment variables for full control

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `BITCOIN_NETWORK` | Preferred network (mainnet/testnet/regtest/signet) | None (uses first available) |
| `BITCOIN_RPC_USER` | RPC username | `test` |
| `BITCOIN_RPC_PASSWORD` | RPC password | `test` |
| `BITCOIN_RPC_HOST` | RPC host | `127.0.0.1` |
| `BITCOIN_RPC_PORT` | Custom RPC port to check | None (uses common ports) |
| `CORE_PATH` | Path to Bitcoin Core source/build | Auto-discovered |

## Network Switching

The system detects the network automatically. If you want to switch networks:

1. **Stop current node** (if you started it)
2. **Start node on desired network**
3. **Set `BITCOIN_NETWORK` environment variable**
4. **Run tests**

The system will find and use the node on the specified network.

## Example: CI Runner Setup

```bash
# On your CI runner, start a persistent regtest node
bitcoind -regtest -daemon \
  -rpcuser=ci \
  -rpcpassword=ci \
  -rpcport=18443 \
  -datadir=/var/lib/bitcoin-regtest

# In your CI workflow
export BITCOIN_NETWORK=regtest
export BITCOIN_RPC_USER=ci
export BITCOIN_RPC_PASSWORD=ci
cargo test --features differential
```

The tests will automatically use the persistent node, making CI runs much faster!

