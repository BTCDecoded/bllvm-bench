# Handling Existing bitcoind Instances

## Current Behavior

### Differential Tests (Rust)

**What it does:**
1. ✅ **Finds Core binaries** - Will detect bitcoind even if already running (checks PATH, CORE_PATH, common locations)
2. ✅ **Avoids port conflicts** - `PortManager` checks if ports are available before using them
3. ✅ **Isolated regtest nodes** - Always starts its own regtest node with unique data directory
4. ✅ **Won't interfere with mainnet/signet** - Uses different ports (18443-18543 range for regtest)

**What it doesn't do:**
- ❌ **Doesn't reuse existing regtest nodes** - Always starts fresh nodes
- ⚠️ **May conflict if regtest on same port** - But PortManager should find available port

### Shell Script Benchmarks

**What they do:**
1. ✅ **Check if bitcoind is running** - Some scripts check for existing instances
2. ✅ **Use existing if available** - Scripts like `performance-rpc-http.sh` will use existing bitcoind
3. ⚠️ **May start new if not found** - Will start regtest node if none exists

## Port Management

The `PortManager` in differential tests:
- Base port: 18443 (regtest default)
- Range: 18443-18543 (100 ports)
- Checks availability: Tries to bind to port before using
- Auto-increments: If port in use, tries next port

**This means:**
- If you have regtest on 18443, tests will use 18444, 18445, etc.
- If you have mainnet (8332) or signet (38332), no conflict (different ports)
- Tests are isolated and won't interfere with your existing nodes

## Potential Issues

1. **Existing regtest on default port**: Tests will find next available port (good)
2. **Port range exhaustion**: If 100+ tests run simultaneously, may need larger range
3. **Data directory conflicts**: Each test uses unique directory (good, but uses disk space)

## Recommendations

### For CI/Runner Setup

If your runner has bitcoind running:

1. **Differential tests**: Will work fine - finds binary, uses available ports
2. **Benchmark scripts**: May try to use existing bitcoind if on correct port, or start new

### Best Practices

1. **Use isolated ports**: Tests use 18443-18543 range, your nodes should use different ports
2. **Mainnet/Signet safe**: Tests only use regtest, won't touch your production nodes
3. **Cleanup**: Tests clean up their data directories after completion (unless `KEEP_REGTEST_DATA=1`)

## Future Improvements

Could add:
- Option to reuse existing regtest node if available
- Better detection of existing nodes
- Configurable port ranges
- Network detection (regtest vs mainnet vs signet)

