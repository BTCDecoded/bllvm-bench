# Troubleshooting Benchmark Failures

## Current Issue: All Commons Benchmarks Failing

All Commons benchmarks are showing `"error": "Benchmark execution failed"` with empty `benchmarks` arrays.

## Likely Causes

1. **Compilation Errors**: `cargo bench` is failing to compile the benchmarks
2. **Missing Dependencies**: Path dependencies (bllvm-consensus, bllvm-node) not found
3. **Wrong Benchmark Names**: Scripts trying non-existent benchmarks
4. **Feature Flags**: `--features production` causing compilation issues

## Fixes Applied

1. ✅ Fixed `CRITERION_DIR` used before definition in `block-validation-bench.sh`
2. ✅ Removed non-existent `connect_block` benchmark name
3. ✅ Made scripts try without `--features production` first, then with it as fallback
4. ✅ Updated benchmark names to match `Cargo.toml` exactly

## How to Debug

1. **Check log files** (mentioned in JSON):
   - `/tmp/block_validation_bench.log`
   - `/tmp/commons-mempool.log`
   - `/tmp/commons-tx-validation.log`

2. **Run a benchmark manually**:
   ```bash
   cd /path/to/bllvm-bench
   cargo bench --bench block_validation_realistic
   ```

3. **Check if dependencies exist**:
   ```bash
   ls -la ../bllvm-consensus/Cargo.toml
   ls -la ../bllvm-node/Cargo.toml
   ```

4. **List available benchmarks**:
   ```bash
   cargo bench --list
   ```

## Next Steps

1. Check the actual error messages in the log files
2. Verify path dependencies are correctly set up
3. Ensure benchmarks compile successfully
4. Re-run benchmarks after fixes
