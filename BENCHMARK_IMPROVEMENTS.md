# Benchmark Improvements & New Benchmarks

## 🎉 Progress Made

- **12 comparisons detected!** (up from 2)
- **13 Core benchmarks** generated (though many are error JSONs - need to fix `bench_bitcoin` build)
- **22 Commons benchmarks** generated
- **RPC HTTP benchmark working** - Core RPC via HTTP measured successfully
- **Memory efficiency benchmark working** - Commons memory usage measured

## 🔧 Critical Issues to Fix First

1. **`bench_bitcoin` build failing** - CMake not finding Makefile (needs proper build step)
2. **Commons path dependencies** - `bllvm-consensus` not found at relative path (symlink issue in workflow)
3. **RPC server startup** - Commons RPC server timing out (needs investigation)

## 📊 New Benchmarks to Add

### High Priority - Fair Comparisons (Both Exist)

#### 1. **Transaction Signing** ⭐⭐⭐
- **Core**: `SignTransactionECDSA`, `SignTransactionSchnorr` (exists in bench_bitcoin)
- **Commons**: Need to check if exists, add if missing
- **Fairness**: Both sign same transaction with same signature type
- **Value**: Critical for wallet/transaction creation performance

#### 2. **Compact Block Encoding** ⭐⭐⭐
- **Core**: `BlockEncodingLargeExtra`, `BlockEncodingNoExtra`, `BlockEncodingStdExtra`
- **Commons**: `benches/node/compact_blocks.rs` exists
- **Fairness**: Both encode same block using BIP152 compact block format
- **Value**: Network efficiency - compact blocks reduce bandwidth

#### 3. **Merkle Root Calculation** ⭐⭐
- **Core**: `MerkleRoot` benchmark
- **Commons**: Has merkle tree benchmarks but may need specific `MerkleRoot` benchmark
- **Fairness**: Both calculate merkle root from same set of transaction hashes
- **Value**: Block validation performance

#### 4. **Block Deserialization** ⭐⭐
- **Core**: `DeserializeBlockTest`, `DeserializeAndCheckBlockTest`
- **Commons**: Need to check if exists
- **Fairness**: Both deserialize same block bytes
- **Value**: Network sync performance

### Medium Priority - Commons-Only (Deep Analysis)

#### 5. **Storage Operations** ⭐⭐
- **Commons**: `benches/node/storage_operations.rs` exists
- **Core**: No direct equivalent (uses LevelDB directly)
- **Value**: Database performance comparison (redb/sled vs LevelDB)
- **Note**: Not a direct comparison, but valuable for Commons optimization

#### 6. **Parallel Block Validation** ⭐⭐⭐
- **Commons**: `benches/node/parallel_block_validation.rs` exists
- **Core**: Sequential only
- **Value**: Shows Commons architectural advantage
- **Status**: Already has script, just needs path fix

#### 7. **Transport Comparison** ⭐
- **Commons**: `benches/node/transport_comparison.rs` exists
- **Value**: Network layer performance (TCP vs QUIC)
- **Note**: Commons-only, but useful for network optimization

#### 8. **Dandelion++** ⭐
- **Commons**: `benches/node/dandelion_bench.rs` exists
- **Core**: No equivalent (privacy feature)
- **Value**: Privacy relay performance
- **Note**: Commons-only feature

#### 9. **UTXO Commitments** ⭐
- **Commons**: `benches/consensus/utxo_commitments.rs` exists
- **Core**: No equivalent
- **Value**: UTXO set commitment performance
- **Note**: Commons-only feature

### Lower Priority - Core-Only (May Not Be Comparable)

#### 10. **Mempool Ephemeral Spends**
- **Core**: `MempoolCheckEphemeralSpends`
- **Commons**: No equivalent
- **Status**: Skip (Core-only feature)

#### 11. **Orphan Transaction Handling**
- **Core**: `OrphanageEraseForBlock`, `OrphanageEraseForPeer`, etc.
- **Commons**: No equivalent
- **Status**: Skip (Core-only feature)

#### 12. **Block Filter Index**
- **Core**: `BlockFilterIndexSync`, `GCSFilter*`
- **Commons**: No equivalent
- **Status**: Skip (Core-only feature)

#### 13. **Transaction Graph**
- **Core**: `TxGraphTrim`
- **Commons**: No equivalent
- **Status**: Skip (Core-only feature)

## 🎯 Recommended Implementation Order

1. **Fix existing issues** (bench_bitcoin build, path dependencies, RPC server)
2. **Add Transaction Signing** - High value, likely both exist
3. **Add Compact Block Encoding** - High value, both exist
4. **Add Merkle Root** - Medium value, likely both exist
5. **Add Storage Operations** - Commons-only, but valuable
6. **Fix Parallel Block Validation** - Already exists, just needs path fix

## 📝 Implementation Notes

### Transaction Signing
- Check if Commons has signing benchmarks
- If missing, add to `bllvm-consensus` or `bllvm-bench`
- Create comparison script similar to `transaction-sighash-bench.sh`

### Compact Block Encoding
- Verify Commons `compact_blocks.rs` benchmark exists and works
- Create comparison script
- Ensure both use same block data

### Merkle Root
- Verify Commons merkle tree benchmarks include root calculation
- May need to add specific `MerkleRoot` benchmark if missing
- Create comparison script

### Storage Operations
- Commons-only benchmark
- Compare redb vs sled performance
- Document as Commons optimization benchmark

## 🔍 What We Learned

1. **Path discovery working** - All paths found correctly
2. **JSON generation working** - Consolidated JSON created successfully
3. **Comparison detection working** - 12 comparisons found automatically
4. **Error handling working** - Failed benchmarks still generate error JSON
5. **Core build needs work** - CMake build step needs fixing
6. **Commons dependencies need work** - Symlink strategy needs improvement

