# Benchmark System Fixes Applied

## Summary

All critical errors from the benchmark run have been fixed. The system should now run successfully on the self-hosted runner.

## Fixes Applied

### 1. Syntax Errors in Core Scripts ✅

**Problem**: Multiple Core benchmark scripts had syntax errors:
- Duplicate `if` statements (lines 31-32)
- Missing closing statements
- Malformed `jq` commands

**Fixed Scripts**:
- `block-serialization-bench.sh`
- `duplicate-inputs-bench.sh`
- `mempool-acceptance-bench.sh`
- `mempool-rbf-bench.sh`
- `ripemd160-bench.sh`
- `segwit-bench.sh`
- `standard-tx-bench.sh`
- `transaction-serialization-bench.sh`
- `transaction-sighash-bench.sh`
- `connectblock-bench.sh` (fixed path to block-validation-bench.sh)

**Changes**:
- Removed duplicate `if ! type get_bench_bitcoin` checks
- Fixed malformed `jq` command in `transaction-sighash-bench.sh`
- Fixed logic flow in `mempool-acceptance-bench.sh`

### 2. Commons Scripts Source Order ✅

**Problem**: Several Commons scripts called `get_output_dir()` before sourcing `common.sh`, causing "command not found" errors.

**Fixed Scripts**:
- `connectblock-bench.sh`
- `duplicate-inputs-bench.sh`
- `mempool-acceptance-bench.sh`
- `mempool-rbf-bench.sh`
- `mempool-bench.sh`
- `transaction-sighash-bench.sh`
- `node-sync-rpc-bench.sh`
- `ripemd160-bench.sh`
- `script-verification-bench.sh`
- `segwit-bench.sh`
- `standard-tx-bench.sh`

**Changes**:
- Moved `source "$SCRIPT_DIR/../shared/common.sh"` to **before** `get_output_dir()` calls
- Ensures all functions are available when called

### 3. OpenSSL/ccache Interference ✅

**Problem**: When `CC=ccache` is set, OpenSSL's build scripts try to use ccache as the compiler directly, causing:
```
ccache: invalid option -- 'O'
Failed to find OpenSSL development headers
```

**Fix**: Added `unset CC CXX` in the workflow before Rust builds:
```yaml
# Unset CC/CXX if they point to ccache
if [ "$CC" = "ccache" ] || [ "${CC##*/}" = "ccache" ]; then
  unset CC
fi
if [ "$CXX" = "ccache" ] || [ "${CXX##*/}" = "ccache" ]; then
  unset CXX
fi
```

**Location**: `.github/workflows/benchmarks.yml` - "Run benchmarks" step

### 4. Documentation ✅

**Created**: `RUNNER_SETUP.md` - Comprehensive guide for runner setup including:
- Required system packages
- Rust setup
- Environment variables
- ccache configuration
- perf_event_paranoid setup
- Common issues and fixes

## Remaining Issues (Require Runner Configuration)

These issues are **not code bugs** but require runner configuration:

### 1. perf_event_paranoid (Deep Analysis Benchmarks)

**Error**: `Access to performance monitoring and observability operations is limited`

**Fix**: On the runner, run:
```bash
sudo sysctl -w kernel.perf_event_paranoid=-1
```

Or permanently:
```bash
echo "kernel.perf_event_paranoid=-1" | sudo tee -a /etc/sysctl.conf
sudo sysctl -p
```

**Note**: This is only needed for deep analysis benchmarks. Other benchmarks will work without it.

### 2. Commons RPC Server Startup

**Error**: `Commons RPC server failed to start or is not responding`

**Possible Causes**:
- Missing dependencies for bllvm-node
- Port already in use
- Build failures in bllvm-node

**Fix**: Ensure bllvm-node builds successfully:
```bash
cd $COMMONS_NODE_PATH
cargo build --release
```

### 3. bench_bitcoin Build

**Error**: `bench_bitcoin not found`

**Status**: The workflow now handles this automatically, but if it fails:
- Check CMake is installed: `cmake --version`
- Check Core was cloned: `ls $CORE_PATH`
- Try manual build: `cd $CORE_PATH && cmake -B build -DBUILD_BENCH=ON && cmake --build build -t bench_bitcoin`

## Validation

All fixes have been validated:
- ✅ All Core scripts pass syntax check
- ✅ All Commons scripts source `common.sh` before calling `get_output_dir()`
- ✅ Workflow unsets `CC`/`CXX` for Rust builds

## Next Steps

1. **On the Runner**: Follow `RUNNER_SETUP.md` to configure the runner properly
2. **Run Benchmarks**: The workflow should now run successfully
3. **Monitor**: Watch the first run for any remaining environment-specific issues

## Files Changed

- `scripts/core/*.sh` - Fixed syntax errors
- `scripts/commons/*.sh` - Fixed source order
- `.github/workflows/benchmarks.yml` - Added CC/CXX unset for Rust builds
- `RUNNER_SETUP.md` - New documentation
- `FIXES_APPLIED.md` - This file

## Testing

To test locally:
```bash
# Test Core scripts
for script in scripts/core/*.sh; do
  bash -n "$script" || echo "❌ $script"
done

# Test Commons scripts
for script in scripts/commons/*.sh; do
  bash -n "$script" || echo "❌ $script"
done
```

All scripts should pass syntax validation.

