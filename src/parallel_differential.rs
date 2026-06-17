//! Parallel Differential Testing
//!
//! This module provides parallel execution of differential tests by splitting
//! the blockchain into chunks and running them concurrently. Each chunk starts
//! from a UTXO checkpoint to enable independent parallel validation.

use anyhow::{Context, Result};
use blvm_protocol::UtxoSet;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

// Re-export block file reader for convenience
pub use crate::block_file_reader::{
    BlockFileReader, Network as BlockFileNetwork, SharedBlockCache,
};

/// Block data source - optimized to avoid RPC when possible
pub enum BlockDataSource {
    /// Direct file reading (fastest - 10-50x faster than RPC)
    DirectFile(BlockFileReader),
    /// Shared cache (fast - 5-10x faster than RPC on subsequent runs)
    SharedCache(
        SharedBlockCache,
        Option<Arc<crate::core_rpc_client::CoreRpcClient>>,
    ),
    /// RPC fallback (slowest but always works)
    Rpc(Arc<crate::core_rpc_client::CoreRpcClient>),
    /// Remote Core RPC via SSH+nsenter (works when datadir files are encrypted)
    RemoteCoreRpc(Arc<crate::remote_core_rpc::RemoteCoreRpcClient>),
}

/// Configuration for parallel differential testing
#[derive(Debug, Clone)]
pub struct ParallelConfig {
    /// Number of parallel workers
    pub num_workers: usize,
    /// Chunk size (blocks per chunk)
    pub chunk_size: u64,
    /// Whether to use UTXO checkpoints (requires sequential pass first)
    pub use_checkpoints: bool,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            num_workers: num_cpus::get(),
            chunk_size: 100_000, // 100k blocks per chunk
            use_checkpoints: true,
        }
    }
}

/// Chunk of blocks to validate
#[derive(Debug, Clone)]
pub struct BlockChunk {
    pub start_height: u64,
    pub end_height: u64,
    pub checkpoint_utxo: Option<UtxoSet>,
    pub skip_validation: bool, // If true, just read blocks for cache building, don't validate
}

/// Result from validating a chunk
#[derive(Debug)]
pub struct ChunkResult {
    pub start_height: u64,
    pub end_height: u64,
    pub tested: usize,
    pub matched: usize,
    pub divergences: Vec<(u64, String, String)>, // (height, blvm_result, core_result)
    pub duration_secs: f64,
}

/// Create optimized block data source
///
/// Tries direct file reading from env-configured Bitcoin Core datadirs first (see
/// [`crate::block_cache_env::bitcoin_data_dir_candidates`]), then remote-Core RPC if `REMOTE_CORE_*` (or legacy `LAND_NODE_*` / `START9_*`) env is set,
/// then shared chunk cache, then standard RPC.
pub fn create_block_data_source(
    network: BlockFileNetwork,
    cache_dir: Option<impl AsRef<std::path::Path>>,
    rpc_client: Option<Arc<crate::core_rpc_client::CoreRpcClient>>,
) -> Result<BlockDataSource> {
    let possible_dirs = crate::block_cache_env::bitcoin_data_dir_candidates();

    for dir in &possible_dirs {
        if !dir.join("blocks").is_dir() {
            continue;
        }
        match BlockFileReader::new(dir, network) {
            Ok(reader) => {
                println!(
                    "✅ Using direct block file reading from BITCOIN_DATA_DIR* {} (fast path)",
                    dir.display()
                );
                return Ok(BlockDataSource::DirectFile(reader));
            }
            Err(e) => {
                eprintln!(
                    "⚠️  Direct file reading from {} failed: {}. Trying next candidate or fallback.",
                    dir.display(),
                    e
                );
            }
        }
    }

    if crate::block_cache_env::remote_core_rpc_env_ready() {
        let remote_core_client = Arc::new(crate::remote_core_rpc::RemoteCoreRpcClient::new());
        println!(
            "✅ Using remote-Core RPC (REMOTE_CORE_* or legacy LAND_NODE_* / START9_* env set; no usable direct datadir from BITCOIN_DATA_DIR*)"
        );
        return Ok(BlockDataSource::RemoteCoreRpc(remote_core_client));
    }

    if let Some(cache_path) = cache_dir {
        let cache = SharedBlockCache::new(cache_path)?;
        println!("✅ Using shared block cache (BLOCK_CACHE_DIR)");
        println!("   Cache will use RPC or populate from your configured sources");
        return Ok(BlockDataSource::SharedCache(cache, rpc_client));
    }

    if let Some(client) = rpc_client {
        println!(
            "⚠️  Using Bitcoin Core RPC only (set BITCOIN_DATA_DIR or BLOCK_CACHE_DIR for faster paths)"
        );
        return Ok(BlockDataSource::Rpc(client));
    }

    anyhow::bail!(
        "No block data source: set BITCOIN_DATA_DIR (and/or BITCOIN_DATA_DIRS) with a blocks/ subdir, \
         or BLOCK_CACHE_DIR for chunks, or REMOTE_CORE_* (or legacy LAND_NODE_* / START9_*) for remote-Core RPC, or run with a reachable Core RPC client."
    )
}

/// Get block data from optimized source
pub async fn get_block_data(source: &BlockDataSource, height: u64) -> Result<Vec<u8>> {
    match source {
        BlockDataSource::DirectFile(reader) => {
            // For direct file reading, we need to iterate sequentially
            // This is a limitation - we'll need to cache blocks or use index
            // For now, fall back to RPC for random access
            anyhow::bail!(
                "Direct file reading requires sequential access. Use generate_checkpoints_sequential or provide RPC client for random access."
            )
        }
        BlockDataSource::SharedCache(cache, rpc_client) => {
            cache
                .get_or_fetch_block(height, rpc_client.as_deref())
                .await
        }
        BlockDataSource::Rpc(client) => {
            let block_hash = client.getblockhash(height).await?;
            let block_hex = client.getblock_raw(&block_hash).await?;
            Ok(hex::decode(&block_hex)?)
        }
        BlockDataSource::RemoteCoreRpc(client) => {
            let block_hash = client.get_block_hash(height).await?;
            let block_hex = client.get_block_hex(&block_hash).await?;
            Ok(hex::decode(&block_hex)?)
        }
    }
}

/// Generate UTXO checkpoints at chunk boundaries
///
/// This runs sequentially to build up UTXO state, then saves checkpoints
/// at chunk boundaries for parallel execution.
///
/// Uses optimized block data source (direct file reading if available).
pub async fn generate_checkpoints(
    start_height: u64,
    end_height: u64,
    chunk_size: u64,
    block_source: &BlockDataSource,
) -> Result<Vec<(u64, UtxoSet)>> {
    use blvm_protocol::block::connect_block;
    use blvm_protocol::segwit::Witness;
    use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
    use blvm_protocol::types::Network;

    // OPTIMIZATION: Pre-allocate checkpoints vector (estimate: ~10 checkpoints for 1M blocks)
    let estimated_checkpoints = ((end_height - start_height) / chunk_size + 1) as usize;
    let mut checkpoints = Vec::with_capacity(estimated_checkpoints.min(100));
    let mut utxo_set = UtxoSet::default();
    let mut previous_block_hash: Option<[u8; 32]> = None; // Track previous block hash for verification

    // If starting from height 0, we start with empty UTXO set
    // Otherwise, we'd need to load from a previous checkpoint

    // Get chain height (need RPC for this)
    let chain_height = match block_source {
        BlockDataSource::Rpc(client) => client.getblockcount().await?,
        BlockDataSource::RemoteCoreRpc(client) => client.get_block_count().await?,
        BlockDataSource::SharedCache(_, Some(client)) => client.getblockcount().await?,
        _ => {
            // For direct file reading, we don't know chain height
            // Use end_height as estimate
            end_height
        }
    };
    let actual_end = end_height.min(chain_height);

    println!(
        "🔧 Generating UTXO checkpoints from {} to {} (chunk size: {})",
        start_height, actual_end, chunk_size
    );

    let mut next_checkpoint = start_height + chunk_size;

    // Use optimized block reading for sequential access
    match block_source {
        BlockDataSource::DirectFile(reader) => {
            // Direct file reading - sequential iterator (fastest!)
            println!("📂 Using direct file reading for checkpoint generation");
            let iterator = reader.read_blocks_sequential(
                Some(start_height),
                Some((actual_end - start_height + 1) as usize),
            )?;
            println!("✅ Iterator created, starting block processing...");

            let mut last_log_time = std::time::Instant::now();
            let mut blocks_processed = 0u64;

            for (idx, block_result) in iterator.enumerate() {
                let height = start_height + idx as u64;

                // CRITICAL: Log every block for first 100, then every 10, then every 1000
                // This ensures we can see exactly where it gets stuck
                if height < 100 {
                    println!("   🔄 [{}] Getting block {} from iterator...", idx, height);
                } else if height < 1000 && height % 10 == 0 {
                    println!("   🔄 [{}] Getting block {} from iterator...", idx, height);
                } else if height % 1000 == 0 {
                    println!("   🔄 [{}] Getting block {} from iterator...", idx, height);
                }

                // Timeout detection - if we haven't made progress in 30 seconds, log warning
                let now = std::time::Instant::now();
                if now.duration_since(last_log_time).as_secs() > 30 && blocks_processed > 0 {
                    eprintln!(
                        "   ⚠️  WARNING: No progress for 30+ seconds! Last block: {}",
                        height - 1
                    );
                    eprintln!("   ⚠️  Iterator may be stuck. Current index: {}", idx);
                    last_log_time = now;
                }

                let block_bytes = match block_result {
                    Ok(bytes) => {
                        if height < 100 {
                            println!(
                                "   ✅ [{}] Got block {} ({} bytes)",
                                idx,
                                height,
                                bytes.len()
                            );
                        }
                        bytes
                    }
                    Err(e) => {
                        eprintln!("❌ Failed to read block at height {}: {}", height, e);
                        return Err(e.into());
                    }
                };

                blocks_processed += 1;
                last_log_time = now;

                // Validate block size
                if block_bytes.len() < 80 {
                    anyhow::bail!(
                        "Block {} too small: {} bytes (minimum 80 for header)",
                        height,
                        block_bytes.len()
                    );
                }

                // Verify previous block hash matches (if not genesis) - this helps detect block boundary issues
                if height > 0 {
                    let prev_hash_in_header = &block_bytes[4..36]; // Previous block hash is at bytes 4-36 (little-endian)
                    // We'll verify this after parsing the block
                }

                if height < 100 {
                    println!("   🔄 [{}] Deserializing block {}...", idx, height);
                }

                let (block, witnesses) = match deserialize_block_with_witnesses(&block_bytes) {
                    Ok(result) => {
                        if height < 100 {
                            println!(
                                "   ✅ [{}] Deserialized block {} ({} txs)",
                                idx,
                                height,
                                result.0.transactions.len()
                            );
                        }
                        result
                    }
                    Err(e) => {
                        eprintln!("❌ Failed to deserialize block at height {}: {}", height, e);
                        eprintln!("   Block size: {} bytes", block_bytes.len());
                        eprintln!(
                            "   First 80 bytes (header, hex): {}",
                            hex::encode(&block_bytes[0..80.min(block_bytes.len())])
                        );
                        if block_bytes.len() > 80 {
                            eprintln!(
                                "   Bytes 80-100 (hex): {}",
                                hex::encode(&block_bytes[80..100.min(block_bytes.len())])
                            );
                        }
                        // For XOR-packaged remote block files, if deserialization fails, the block boundary might be wrong
                        // Try to continue - this will help us identify all problematic blocks
                        eprintln!(
                            "⚠️  Block {} deserialization failed - likely block boundary issue. Skipping.",
                            height
                        );
                        continue; // Skip this block and continue
                    }
                };

                // Debug: Check if previous blocks had non-coinbase transactions
                if height <= 16 {
                    let non_coinbase_count = block
                        .transactions
                        .iter()
                        .filter(|tx| !blvm_protocol::transaction::is_coinbase(tx))
                        .count();
                    if non_coinbase_count > 0 {
                        eprintln!(
                            "🔍 Block {}: {} non-coinbase transactions",
                            height, non_coinbase_count
                        );
                        // For each non-coinbase transaction, show what it's spending
                        for (tx_idx, tx) in block.transactions.iter().enumerate() {
                            if !blvm_protocol::transaction::is_coinbase(tx) {
                                use blvm_protocol::block::calculate_tx_id;
                                let txid = calculate_tx_id(tx);
                                let txid_str: String =
                                    txid.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                                eprintln!(
                                    "   TX {} (non-coinbase): {} inputs, {} outputs, TXID: {}...",
                                    tx_idx,
                                    tx.inputs.len(),
                                    tx.outputs.len(),
                                    txid_str
                                );
                                if !tx.inputs.is_empty() {
                                    let hash_str: String = tx.inputs[0]
                                        .prevout
                                        .hash
                                        .iter()
                                        .take(8)
                                        .map(|b| format!("{:02x}", b))
                                        .collect();
                                    eprintln!(
                                        "      Spending: {}:{}",
                                        hash_str, tx.inputs[0].prevout.index
                                    );
                                }
                            }
                        }
                    }
                }

                // Debug: Check UTXO set after each block to see if outputs are being added
                if height <= 16 {
                    let non_coinbase_utxos: Vec<_> = utxo_set
                        .iter()
                        .filter(|(_, utxo)| !utxo.is_coinbase)
                        .collect();
                    if !non_coinbase_utxos.is_empty() {
                        eprintln!(
                            "🔍 After block {}: {} non-coinbase UTXOs in set",
                            height,
                            non_coinbase_utxos.len()
                        );
                        for (outpoint, utxo) in non_coinbase_utxos.iter().take(3) {
                            let hash_str: String = outpoint
                                .hash
                                .iter()
                                .take(8)
                                .map(|b| format!("{:02x}", b))
                                .collect();
                            eprintln!(
                                "   Non-coinbase UTXO: {}:{} (value={}, height={})",
                                hash_str, outpoint.index, utxo.value, utxo.height
                            );
                        }
                    }
                }

                // Debug: Print transaction details for block 15
                if height == 15 {
                    eprintln!(
                        "🔍 DEBUG Block 15: {} transactions",
                        block.transactions.len()
                    );
                    eprintln!("   UTXO set size: {}", utxo_set.len());
                    // List all UTXOs in the set
                    eprintln!("   All UTXOs in set:");
                    for (outpoint, utxo) in utxo_set.iter() {
                        let hash_str: String = outpoint
                            .hash
                            .iter()
                            .take(8)
                            .map(|b| format!("{:02x}", b))
                            .collect();
                        eprintln!(
                            "      {}:{} (value={}, height={}, coinbase={})",
                            hash_str, outpoint.index, utxo.value, utxo.height, utxo.is_coinbase
                        );
                    }
                    for (tx_idx, tx) in block.transactions.iter().enumerate() {
                        eprintln!(
                            "   TX {}: {} inputs, {} outputs",
                            tx_idx,
                            tx.inputs.len(),
                            tx.outputs.len()
                        );
                        if !tx.inputs.is_empty() {
                            let hash_str: String = tx.inputs[0]
                                .prevout
                                .hash
                                .iter()
                                .take(8)
                                .map(|b| format!("{:02x}", b))
                                .collect();
                            eprintln!(
                                "      First input prevout: {}:{}",
                                hash_str, tx.inputs[0].prevout.index
                            );
                            // Check if UTXO exists
                            if let Some(utxo) = utxo_set.get(&tx.inputs[0].prevout) {
                                eprintln!(
                                    "      UTXO exists: value={}, height={}, coinbase={}",
                                    utxo.value, utxo.height, utxo.is_coinbase
                                );
                            } else {
                                eprintln!("      UTXO MISSING!");
                                // The prevout hash should be a transaction ID from a previous block
                                // Let's check if we can find it in the UTXO set by searching for matching txids
                                eprintln!("      Looking for TX that created this UTXO...");
                                let target_hash = tx.inputs[0].prevout.hash;
                                let target_index = tx.inputs[0].prevout.index;
                                let mut found_match = false;
                                for (outpoint, utxo) in utxo_set.iter() {
                                    if outpoint.hash == target_hash {
                                        found_match = true;
                                        let hash_str: String = outpoint
                                            .hash
                                            .iter()
                                            .take(8)
                                            .map(|b| format!("{:02x}", b))
                                            .collect();
                                        eprintln!(
                                            "      Found matching TX ID in UTXO set: {}:{} (target index: {})",
                                            hash_str, outpoint.index, target_index
                                        );
                                        eprintln!(
                                            "      UTXO details: value={}, height={}, coinbase={}",
                                            utxo.value, utxo.height, utxo.is_coinbase
                                        );
                                    }
                                }
                                if !found_match {
                                    let hash_str: String = target_hash
                                        .iter()
                                        .take(8)
                                        .map(|b| format!("{:02x}", b))
                                        .collect();
                                    eprintln!(
                                        "      No UTXO found with TX ID: {} (index: {})",
                                        hash_str, target_index
                                    );
                                    eprintln!(
                                        "      This UTXO should have been created in a previous block"
                                    );
                                    // Check if this TX ID matches any coinbase TX ID in the UTXO set
                                    eprintln!(
                                        "      Checking if this matches any coinbase TX ID..."
                                    );
                                    let mut found_coinbase_match = false;
                                    for (outpoint, utxo) in utxo_set.iter() {
                                        if utxo.is_coinbase {
                                            let outpoint_hash_str: String = outpoint
                                                .hash
                                                .iter()
                                                .take(8)
                                                .map(|b| format!("{:02x}", b))
                                                .collect();
                                            if outpoint.hash == target_hash {
                                                found_coinbase_match = true;
                                                eprintln!(
                                                    "      ✅ Found matching coinbase TX ID: {}:{} (but index {} doesn't match)",
                                                    outpoint_hash_str, outpoint.index, target_index
                                                );
                                                eprintln!(
                                                    "      This suggests the transaction is trying to spend the wrong output index"
                                                );
                                                break;
                                            }
                                        }
                                    }
                                    if !found_coinbase_match {
                                        eprintln!(
                                            "      ❌ No matching coinbase TX ID found - this UTXO was never created"
                                        );
                                    }
                                }
                            }
                        }
                        // Calculate TX ID
                        use blvm_protocol::block::calculate_tx_id;
                        let txid = calculate_tx_id(tx);
                        let txid_str: String =
                            txid.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                        eprintln!("      TX ID: {}...", txid_str);
                    }
                }

                // Calculate this block's hash for next block verification
                // OPTIMIZATION: Cache hash calculation (only compute once per block)
                use sha2::{Digest, Sha256};
                let header = &block_bytes[0..80];
                let first_hash = Sha256::digest(header);
                let second_hash = Sha256::digest(&first_hash);
                // OPTIMIZATION: Use array directly instead of Vec allocation
                let mut current_block_hash: [u8; 32] = second_hash
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("Invalid hash length"))?;
                current_block_hash.reverse(); // Convert to big-endian

                // Verify previous block hash matches (if not genesis)
                // For XOR-packaged block files, if prev hash doesn't match, the block boundary detection
                // is likely wrong. We'll skip validation errors for now and continue to identify
                // which blocks work correctly.
                if height > 0 {
                    let prev_hash_in_header = &block_bytes[4..36]; // Previous block hash is at bytes 4-36 (little-endian, as stored)
                    if let Some(prev_hash) = previous_block_hash {
                        // Convert previous_block_hash (big-endian) to little-endian for comparison
                        let prev_hash_le: Vec<u8> = prev_hash.iter().rev().copied().collect();
                        if prev_hash_in_header != prev_hash_le.as_slice() {
                            // This indicates we're reading too much data - block boundary is wrong
                            eprintln!(
                                "⚠️  Block {}: Previous block hash mismatch - block boundary detection issue!",
                                height
                            );
                            eprintln!("   Header has (LE): {}", hex::encode(prev_hash_in_header));
                            eprintln!("   Expected (LE):   {}", hex::encode(&prev_hash_le));
                            eprintln!(
                                "   Block size: {} bytes (likely reading too much - should use size field or verify hash)",
                                block_bytes.len()
                            );
                        }
                    }
                }

                // Update previous block hash for next iteration
                previous_block_hash = Some(current_block_hash);

                // Debug: Check transaction count and verify block hash for problematic blocks
                if height == 15 || height == 10 {
                    let block_hash_hex = hex::encode(&current_block_hash[..8]);
                    eprintln!(
                        "DEBUG Block {}: Parsed {} transactions, block hash (first 8 bytes) = {}, block size = {} bytes",
                        height,
                        block.transactions.len(),
                        block_hash_hex,
                        block_bytes.len()
                    );
                    for (i, tx) in block.transactions.iter().enumerate() {
                        eprintln!(
                            "DEBUG Block {}: TX {} has {} inputs, {} outputs",
                            height,
                            i,
                            tx.inputs.len(),
                            tx.outputs.len()
                        );
                    }
                }

                // Debug: Verify we're calculating the correct coinbase txid
                #[cfg(debug_assertions)]
                if height <= 2 {
                    use blvm_protocol::block::calculate_tx_id;
                    if let Some(coinbase) = block.transactions.first() {
                        let txid = calculate_tx_id(coinbase);
                        eprintln!(
                            "DEBUG Block {}: coinbase txid = {}",
                            height,
                            hex::encode(txid)
                        );
                        eprintln!("DEBUG Block {}: UTXO set size = {}", height, utxo_set.len());
                        // List all coinbase UTXOs in the set
                        let mut coinbase_utxos = Vec::new();
                        for (outpoint, utxo) in utxo_set.iter() {
                            if utxo.is_coinbase {
                                coinbase_utxos.push((hex::encode(outpoint.hash), utxo.height));
                            }
                        }
                        if !coinbase_utxos.is_empty() {
                            eprintln!(
                                "DEBUG Block {}: Coinbase UTXOs in set: {:?}",
                                height, coinbase_utxos
                            );
                        }
                    }
                }

                // Validate with BLVM
                if height < 100 {
                    println!(
                        "   🔄 [{}] Calling connect_block for block {}...",
                        idx, height
                    );
                }

                let ctx = blvm_protocol::block::block_validation_context_for_connect_ibd(
                    None::<&[blvm_protocol::types::BlockHeader]>,
                    block.header.timestamp,
                    Network::Mainnet,
                );
                let connect_start = std::time::Instant::now();
                let (result, new_utxo_set, _undo_log) =
                    connect_block(&block, &witnesses, utxo_set.clone(), height, &ctx)?;

                let connect_duration = connect_start.elapsed();
                if height < 100 {
                    println!(
                        "   ✅ [{}] connect_block completed for block {} in {:.2}ms",
                        idx,
                        height,
                        connect_duration.as_millis()
                    );
                } else if connect_duration.as_secs() > 1 {
                    eprintln!(
                        "   ⚠️  [{}] connect_block took {:.2}s for block {} (slow!)",
                        idx,
                        connect_duration.as_secs_f64(),
                        height
                    );
                }

                if matches!(result, blvm_protocol::types::ValidationResult::Valid) {
                    utxo_set = new_utxo_set;
                    if height < 100 {
                        println!(
                            "   ✅ [{}] Block {} validated successfully, UTXO set size: {}",
                            idx,
                            height,
                            utxo_set.len()
                        );
                    }
                } else {
                    // OPTIMIZATION: Use string reference instead of clone
                    let error_msg = match &result {
                        blvm_protocol::types::ValidationResult::Invalid(msg) => msg.as_str(),
                        _ => "Unknown error",
                    };
                    eprintln!("❌ Block {} validation failed: {}", height, error_msg);
                    anyhow::bail!(
                        "Block {} failed validation during checkpoint generation: {}",
                        height,
                        error_msg
                    );
                }

                // Save checkpoint at chunk boundaries
                // CRITICAL: Save checkpoint at the END of each chunk (before the next chunk starts)
                // For chunk 0-169, save at height 169 (after processing block 169)
                // For chunk 170-339, save at height 339 (after processing block 339)
                // This ensures the checkpoint contains UTXOs from blocks 0-169, not 0-170
                if height == next_checkpoint - 1 || height == actual_end {
                    println!(
                        "✅ Checkpoint at height {} (UTXO count: {})",
                        height,
                        utxo_set.len()
                    );
                    // NOTE: Must clone here because we continue processing after checkpoint
                    checkpoints.push((height, utxo_set.clone()));
                    next_checkpoint += chunk_size;
                }

                // Progress indicator - more frequent for early blocks to catch issues
                if height < 100 && height % 10 == 0 {
                    println!(
                        "📊 Checkpoint generation: {}/{} ({:.1}%)",
                        height - start_height,
                        actual_end - start_height,
                        100.0 * (height - start_height) as f64 / (actual_end - start_height) as f64
                    );
                } else if height < 1000 && height % 100 == 0 {
                    println!(
                        "📊 Checkpoint generation: {}/{} ({:.1}%)",
                        height - start_height,
                        actual_end - start_height,
                        100.0 * (height - start_height) as f64 / (actual_end - start_height) as f64
                    );
                } else if height % 10_000 == 0 {
                    println!(
                        "📊 Checkpoint generation: {}/{} ({:.1}%)",
                        height - start_height,
                        actual_end - start_height,
                        100.0 * (height - start_height) as f64 / (actual_end - start_height) as f64
                    );
                }

                if height < 100 {
                    println!(
                        "   ✅ [{}] Finished processing block {}, moving to next...",
                        idx, height
                    );
                }
            }
        }
        _ => {
            // For cache/RPC, fetch blocks sequentially (async)
            for height in start_height..=actual_end {
                let block_bytes = get_block_data(block_source, height).await?;

                let (block, witnesses) = deserialize_block_with_witnesses(&block_bytes)?;

                // DEBUG: Always log BIP30-relevant info for early blocks to diagnose issue
                if height <= 10 {
                    use blvm_protocol::block::calculate_tx_id;
                    use sha2::{Digest, Sha256};

                    if let Some(coinbase) = block.transactions.first() {
                        let txid = calculate_tx_id(coinbase);
                        eprintln!(
                            "   📍 DEBUG Block {}: coinbase txid (first 8) = {}",
                            height,
                            hex::encode(&txid[..8])
                        );
                        eprintln!(
                            "   📍 DEBUG Block {}: UTXO set size before connect_block = {}",
                            height,
                            utxo_set.len()
                        );

                        // Check for matching UTXOs (what BIP30 will check)
                        let mut matches = Vec::new();
                        for (outpoint, utxo) in utxo_set.iter() {
                            if outpoint.hash == txid {
                                matches.push((outpoint.index, utxo.is_coinbase, utxo.height));
                            }
                        }
                        if !matches.is_empty() {
                            eprintln!(
                                "   📍 DEBUG Block {}: Found {} UTXO(s) with matching txid:",
                                height,
                                matches.len()
                            );
                            for (idx, is_cb, h) in matches.iter().take(5) {
                                eprintln!(
                                    "      OutPoint index: {}, is_coinbase: {}, height: {}",
                                    idx, is_cb, h
                                );
                            }
                        } else {
                            eprintln!(
                                "   📍 DEBUG Block {}: No UTXOs with matching txid found",
                                height
                            );
                        }
                    }
                }

                let ctx = blvm_protocol::block::block_validation_context_for_connect_ibd(
                    None::<&[blvm_protocol::types::BlockHeader]>,
                    block.header.timestamp,
                    Network::Mainnet,
                );
                let (result, new_utxo_set, _undo_log) =
                    connect_block(&block, &witnesses, utxo_set.clone(), height, &ctx)?;

                if matches!(result, blvm_protocol::types::ValidationResult::Valid) {
                    utxo_set = new_utxo_set;
                } else {
                    // OPTIMIZATION: Use string reference instead of clone
                    let error_msg = match &result {
                        blvm_protocol::types::ValidationResult::Invalid(msg) => msg.as_str(),
                        _ => "Unknown error",
                    };
                    eprintln!("❌ Block {} validation failed: {}", height, error_msg);
                    anyhow::bail!(
                        "Block {} failed validation during checkpoint generation: {}",
                        height,
                        error_msg
                    );
                }

                // Save checkpoint at chunk boundaries
                // CRITICAL: Save checkpoint at the END of each chunk (before the next chunk starts)
                // For chunk 0-169, save at height 169 (after processing block 169)
                // For chunk 170-339, save at height 339 (after processing block 339)
                // This ensures the checkpoint contains UTXOs from blocks 0-169, not 0-170
                if height == next_checkpoint - 1 || height == actual_end {
                    println!(
                        "✅ Checkpoint at height {} (UTXO count: {})",
                        height,
                        utxo_set.len()
                    );
                    // NOTE: Must clone here because we continue processing after checkpoint
                    // The checkpoint is saved for parallel validation later
                    checkpoints.push((height, utxo_set.clone()));
                    next_checkpoint += chunk_size;
                }

                // Progress indicator - more frequent for early blocks to catch issues
                if height < 100 && height % 10 == 0 {
                    println!(
                        "📊 Checkpoint generation: {}/{} ({:.1}%)",
                        height - start_height,
                        actual_end - start_height,
                        100.0 * (height - start_height) as f64 / (actual_end - start_height) as f64
                    );
                } else if height < 1000 && height % 100 == 0 {
                    println!(
                        "📊 Checkpoint generation: {}/{} ({:.1}%)",
                        height - start_height,
                        actual_end - start_height,
                        100.0 * (height - start_height) as f64 / (actual_end - start_height) as f64
                    );
                } else if height % 10_000 == 0 {
                    println!(
                        "📊 Checkpoint generation: {}/{} ({:.1}%)",
                        height - start_height,
                        actual_end - start_height,
                        100.0 * (height - start_height) as f64 / (actual_end - start_height) as f64
                    );
                }
            }
        }
    }

    Ok(checkpoints)
}

/// Process a single block (validate with BLVM and Core)
///
/// Uses remote-Core RPC for Core validation if available, even when reading from DirectFile/chunks
async fn process_block(
    block_bytes: &[u8],
    height: u64,
    utxo_set: &mut UtxoSet,
    block_source: &BlockDataSource,
) -> Result<(
    crate::differential::ValidationResult,
    crate::differential::CoreValidationResult,
)> {
    use crate::differential::{CoreValidationResult, ValidationResult};

    // OPTIMIZATION: Cache remote-Core RPC client to avoid creating new one for each block
    // Check if remote-Core RPC is available for Core validation (even if using DirectFile for blocks)
    use std::sync::Mutex;
    static REMOTE_CORE_RPC_CLIENT: Mutex<Option<Arc<crate::remote_core_rpc::RemoteCoreRpcClient>>> =
        Mutex::new(None);

    let has_remote_core_rpc = crate::block_cache_env::remote_core_rpc_env_ready();
    use blvm_protocol::block::connect_block;
    use blvm_protocol::segwit::Witness;
    use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
    use blvm_protocol::types::Network;

    let (block, witnesses) = match deserialize_block_with_witnesses(block_bytes) {
        Ok((b, w)) => (b, w),
        Err(e) => {
            anyhow::bail!("Failed to deserialize block at height {}: {}", height, e);
        }
    };

    // DEBUG: Log transaction counts for all blocks 0-20 to see if non-coinbase transactions exist
    if height <= 20 {
        let coinbase_count = block
            .transactions
            .iter()
            .filter(|tx| blvm_protocol::transaction::is_coinbase(tx))
            .count();
        let non_coinbase_count = block.transactions.len() - coinbase_count;
        if non_coinbase_count > 0 {
            eprintln!(
                "   🔍 DEBUG Block {}: {} total transactions ({} coinbase, {} non-coinbase)",
                height,
                block.transactions.len(),
                coinbase_count,
                non_coinbase_count
            );
        }
    }

    // DEBUG: Log UTXO set and transaction details for problematic blocks
    if height == 15
        || height == 17
        || height == 86
        || height == 120
        || height == 126
        || height == 153
        || height == 160
        || height == 318
    {
        eprintln!(
            "   🔍 DEBUG process_block Block {}: UTXO set size before connect_block: {}",
            height,
            utxo_set.len()
        );
        eprintln!("      Block has {} transactions", block.transactions.len());
        // Calculate txids for all transactions in this block
        use blvm_protocol::block::calculate_tx_id;
        let block_txids: Vec<_> = block
            .transactions
            .iter()
            .map(|tx| calculate_tx_id(tx))
            .collect();
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            let txid_str: String = block_txids[tx_idx]
                .iter()
                .take(8)
                .map(|b| format!("{:02x}", b))
                .collect();
            eprintln!("      TX {}: txid (first 8) = {}", tx_idx, txid_str);
            if !blvm_protocol::transaction::is_coinbase(tx) && !tx.inputs.is_empty() {
                for (input_idx, input) in tx.inputs.iter().enumerate() {
                    let hash_str: String = input
                        .prevout
                        .hash
                        .iter()
                        .take(8)
                        .map(|b| format!("{:02x}", b))
                        .collect();
                    eprintln!(
                        "         Input {}: prevout {}:{}",
                        input_idx, hash_str, input.prevout.index
                    );
                    if let Some(utxo) = utxo_set.get(&input.prevout) {
                        eprintln!(
                            "            ✅ UTXO exists in utxo_set: value={}, height={}, coinbase={}",
                            utxo.value, utxo.height, utxo.is_coinbase
                        );
                    } else {
                        eprintln!("            ❌ UTXO MISSING in utxo_set");
                        // Check if it might be from an earlier transaction in this block
                        if tx_idx > 0 {
                            eprintln!(
                                "            🔍 Checking if from earlier transaction in this block..."
                            );
                            for prev_tx_idx in 0..tx_idx {
                                let prev_txid_str: String = block_txids[prev_tx_idx]
                                    .iter()
                                    .take(8)
                                    .map(|b| format!("{:02x}", b))
                                    .collect();
                                eprintln!(
                                    "               Comparing with TX {}: txid (first 8) = {}",
                                    prev_tx_idx, prev_txid_str
                                );
                                if input.prevout.hash == block_txids[prev_tx_idx] {
                                    eprintln!(
                                        "               ✅ MATCH! This is from TX {} in this block, output index: {}",
                                        prev_tx_idx, input.prevout.index
                                    );
                                    eprintln!(
                                        "               🔍 This should be available in temp_utxo_set during validation"
                                    );
                                }
                            }
                        }
                        // Check if it's from a previous block (should be in utxo_set)
                        eprintln!("            🔍 Checking if from previous blocks...");
                        eprintln!(
                            "            🔍 Looking for prevout hash: {} (full 32 bytes)",
                            hex::encode(&input.prevout.hash)
                        );
                        let mut found_in_prev = false;
                        let mut partial_matches = Vec::new();
                        for (outpoint, utxo) in utxo_set.iter() {
                            if outpoint.hash == input.prevout.hash {
                                found_in_prev = true;
                                eprintln!(
                                    "            ✅ Found exact match: {}:{} (value={}, height={}, coinbase={})",
                                    hex::encode(&outpoint.hash[..8]),
                                    outpoint.index,
                                    utxo.value,
                                    utxo.height,
                                    utxo.is_coinbase
                                );
                            } else if outpoint.hash[..8] == input.prevout.hash[..8] {
                                // First 8 bytes match but full hash doesn't - this is suspicious
                                partial_matches.push((
                                    hex::encode(&outpoint.hash[..8]),
                                    outpoint.index,
                                    utxo.height,
                                ));
                            }
                        }
                        if !found_in_prev {
                            if !partial_matches.is_empty() {
                                eprintln!(
                                    "            ⚠️  Found {} partial matches (first 8 bytes match but full hash doesn't):",
                                    partial_matches.len()
                                );
                                for (hash8, idx, h) in partial_matches.iter().take(5) {
                                    eprintln!("               {}:{} (height={})", hash8, idx, h);
                                }
                            }
                            eprintln!("            ❌ Not found in any previous block either");
                            // List all UTXOs in the set to see what we have
                            eprintln!("            📋 All UTXOs in set (first 15):");
                            for (idx, (outpoint, utxo)) in utxo_set.iter().take(15).enumerate() {
                                let op_hash_str: String = outpoint
                                    .hash
                                    .iter()
                                    .take(8)
                                    .map(|b| format!("{:02x}", b))
                                    .collect();
                                eprintln!(
                                    "               {}: {}:{} (value={}, height={}, coinbase={})",
                                    idx,
                                    op_hash_str,
                                    outpoint.index,
                                    utxo.value,
                                    utxo.height,
                                    utxo.is_coinbase
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    let ctx = blvm_protocol::block::block_validation_context_for_connect_ibd(
        None::<&[blvm_protocol::types::BlockHeader]>,
        block.header.timestamp,
        Network::Mainnet,
    );
    let blvm_result = match connect_block(&block, &witnesses, utxo_set.clone(), height, &ctx) {
        Ok((result, new_utxo_set, _undo_log)) => {
            *utxo_set = new_utxo_set;
            match result {
                blvm_protocol::types::ValidationResult::Valid => ValidationResult::Valid,
                blvm_protocol::types::ValidationResult::Invalid(msg) => {
                    ValidationResult::Invalid(msg)
                }
            }
        }
        Err(e) => ValidationResult::Invalid(format!("{:?}", e)),
    };

    // Validate with Core
    // CRITICAL: Use remote-Core RPC if available, even when reading from DirectFile/chunks
    let core_result = if has_remote_core_rpc {
        // Use remote-Core RPC to validate against actual Core node
        use sha2::{Digest, Sha256};
        if block_bytes.len() >= 80 {
            let header = &block_bytes[0..80];
            let first_hash = Sha256::digest(header);
            let second_hash = Sha256::digest(&first_hash);
            let mut hash_bytes: [u8; 32] = second_hash
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid hash length"))?;
            hash_bytes.reverse();
            let block_hash = hex::encode(hash_bytes);

            // OPTIMIZATION: Reuse cached remote-Core RPC client (created once, reused for all blocks)
            let remote_core_client = {
                let mut client_guard = REMOTE_CORE_RPC_CLIENT.lock().unwrap();
                if client_guard.is_none() {
                    *client_guard =
                        Some(Arc::new(crate::remote_core_rpc::RemoteCoreRpcClient::new()));
                }
                client_guard.as_ref().unwrap().clone()
            };
            match remote_core_client.get_block_hex(&block_hash).await {
                Ok(_) => CoreValidationResult::Valid,
                Err(_) => CoreValidationResult::Invalid("Block not in Core chain".to_string()),
            }
        } else {
            CoreValidationResult::Invalid("Block too short".to_string())
        }
    } else {
        // Fallback to source-specific validation
        match block_source {
            BlockDataSource::DirectFile(_) => {
                // No remote-Core RPC available, assume valid for direct file reading
                CoreValidationResult::Valid
            }
            BlockDataSource::SharedCache(_, Some(client)) | BlockDataSource::Rpc(client) => {
                // Calculate block hash to check with Core
                // OPTIMIZATION: Use fixed-size array instead of Vec allocation
                // OPTIMIZATION: Cache hash calculation if called multiple times
                use sha2::{Digest, Sha256};
                if block_bytes.len() >= 80 {
                    let header = &block_bytes[0..80];
                    let first_hash = Sha256::digest(header);
                    let second_hash = Sha256::digest(&first_hash);
                    // Reverse bytes for Core RPC (Core displays hashes in reverse)
                    // OPTIMIZATION: Use array instead of Vec to avoid allocation
                    let mut hash_bytes: [u8; 32] =
                        second_hash.as_slice().try_into().unwrap_or_else(|_| {
                            // Fallback: create zero array if conversion fails
                            [0u8; 32]
                        });
                    hash_bytes.reverse();
                    let block_hash = hex::encode(hash_bytes);

                    match client.getblock(&block_hash, 1).await {
                        Ok(_) => CoreValidationResult::Valid,
                        Err(_) => CoreValidationResult::Invalid("Block not in chain".to_string()),
                    }
                } else {
                    CoreValidationResult::Invalid("Block too short".to_string())
                }
            }
            BlockDataSource::RemoteCoreRpc(client) => {
                // Calculate block hash to check with Core
                // OPTIMIZATION: Use fixed-size array instead of Vec allocation
                use sha2::{Digest, Sha256};
                if block_bytes.len() >= 80 {
                    let header = &block_bytes[0..80];
                    let first_hash = Sha256::digest(header);
                    let second_hash = Sha256::digest(&first_hash);
                    // Reverse bytes for Core RPC (Core displays hashes in reverse)
                    // OPTIMIZATION: Use array instead of Vec to avoid allocation
                    let mut hash_bytes: [u8; 32] = second_hash
                        .as_slice()
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("Invalid hash length"))?;
                    hash_bytes.reverse();
                    let block_hash = hex::encode(hash_bytes);

                    // Remote-Core RPC - just check if we can get the block
                    match client.get_block_hex(&block_hash).await {
                        Ok(_) => CoreValidationResult::Valid,
                        Err(_) => CoreValidationResult::Invalid("Block not in chain".to_string()),
                    }
                } else {
                    CoreValidationResult::Invalid("Block too short".to_string())
                }
            }
            _ => {
                // No RPC client available, assume valid
                CoreValidationResult::Valid
            }
        }
    };

    Ok((blvm_result, core_result))
}

/// Validate a single chunk of blocks
///
/// Uses optimized block data source (direct file reading if available).
pub async fn validate_chunk(
    chunk: BlockChunk,
    block_source: Arc<BlockDataSource>,
) -> Result<ChunkResult> {
    use crate::differential::{CoreValidationResult, ValidationResult};
    use std::time::Instant;

    let start_time = Instant::now();
    let mut utxo_set = chunk.checkpoint_utxo.unwrap_or_default();
    // OPTIMIZATION: Pre-allocate divergences vector (most tests have 0-10 divergences)
    let mut divergences = Vec::with_capacity(10);
    let mut tested = 0;
    let mut matched = 0;

    // Get chain height
    let chain_height = match block_source.as_ref() {
        BlockDataSource::Rpc(client) => client.getblockcount().await?,
        BlockDataSource::RemoteCoreRpc(client) => client.get_block_count().await?,
        BlockDataSource::SharedCache(_, Some(client)) => client.getblockcount().await?,
        BlockDataSource::DirectFile(_) => chunk.end_height, // Don't know exact height
        BlockDataSource::SharedCache(_, None) => chunk.end_height, // Don't know exact height
    };
    let actual_end = chunk.end_height.min(chain_height);

    // Process blocks based on data source
    println!(
        "   📍 DEBUG: validate_chunk: Processing blocks, source type: {:?}",
        std::mem::discriminant(block_source.as_ref())
    );
    match block_source.as_ref() {
        BlockDataSource::DirectFile(reader) => {
            println!("   📍 DEBUG: Using DirectFile source, calling read_blocks_sequential...");
            // Direct file reading - sequential iterator (fastest!)
            let iterator = reader.read_blocks_sequential(
                Some(chunk.start_height),
                Some((actual_end - chunk.start_height + 1) as usize),
            )?;
            println!("   📍 DEBUG: Got iterator, starting to enumerate blocks...");

            for (idx, block_result) in iterator.enumerate() {
                let height = chunk.start_height + idx as u64;
                if idx == 0 {
                    println!("   📍 DEBUG: Processing first block at height {}", height);
                }
                let block_bytes = match block_result {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        eprintln!("   ❌ ERROR: Failed to read block at index {}: {}", idx, e);
                        return Err(e.into());
                    }
                };

                if idx == 0 {
                    println!(
                        "   📍 DEBUG: Got first block ({} bytes), calling process_block...",
                        block_bytes.len()
                    );
                }

                // Process block (same logic for both paths)
                let (blvm_result, core_result) =
                    process_block(&block_bytes, height, &mut utxo_set, block_source.as_ref())
                        .await?;

                // Compare and record results
                let matches = matches!(
                    (&blvm_result, &core_result),
                    (ValidationResult::Valid, CoreValidationResult::Valid)
                        | (
                            ValidationResult::Invalid(_),
                            CoreValidationResult::Invalid(_)
                        )
                );

                if !matches {
                    // OPTIMIZATION: Use format! directly instead of intermediate strings
                    let blvm_str = match &blvm_result {
                        ValidationResult::Valid => "Valid".to_string(),
                        ValidationResult::Invalid(msg) => format!("Invalid({})", msg),
                    };
                    let core_str = match &core_result {
                        CoreValidationResult::Valid => "Valid".to_string(),
                        CoreValidationResult::Invalid(msg) => format!("Invalid({})", msg),
                    };
                    divergences.push((height, blvm_str.clone(), core_str.clone()));
                    eprintln!(
                        "❌ DIVERGENCE at height {}: BLVM={}, Core={}",
                        height, blvm_str, core_str
                    );

                    // Log first few divergences with more detail
                    if divergences.len() <= 5 {
                        use sha2::{Digest, Sha256};
                        if block_bytes.len() >= 80 {
                            let header = &block_bytes[0..80];
                            let first_hash = Sha256::digest(header);
                            let second_hash = Sha256::digest(&first_hash);
                            let mut hash_bytes = second_hash.as_slice().to_vec();
                            hash_bytes.reverse();
                            let block_hash = hex::encode(&hash_bytes[..8]);
                            eprintln!("   Block hash (first 8 bytes): {}", block_hash);
                        }
                    }
                } else {
                    matched += 1;
                }

                tested += 1;

                // Progress indicator every 100 blocks (more frequent for better feedback)
                if tested % 100 == 0 || tested == 1 {
                    let total = actual_end - chunk.start_height + 1;
                    let pct = 100.0 * tested as f64 / total as f64;
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let rate = tested as f64 / elapsed;
                    println!(
                        "📊 Chunk [{}-{}]: {}/{} blocks ({:.1}%) @ {:.1} blocks/sec",
                        chunk.start_height, actual_end, tested, total, pct, rate
                    );
                }
            }
        }
        _ => {
            // For cache/RPC, fetch blocks sequentially (async)
            for height in chunk.start_height..=actual_end {
                let block_bytes = get_block_data(block_source.as_ref(), height).await?;

                // Process block (same logic)
                let (blvm_result, core_result) =
                    process_block(&block_bytes, height, &mut utxo_set, block_source.as_ref())
                        .await?;

                // Compare and record results
                let matches = matches!(
                    (&blvm_result, &core_result),
                    (ValidationResult::Valid, CoreValidationResult::Valid)
                        | (
                            ValidationResult::Invalid(_),
                            CoreValidationResult::Invalid(_)
                        )
                );

                if !matches {
                    // OPTIMIZATION: Use format! directly instead of intermediate strings
                    let blvm_str = match &blvm_result {
                        ValidationResult::Valid => "Valid".to_string(),
                        ValidationResult::Invalid(msg) => format!("Invalid({})", msg),
                    };
                    let core_str = match &core_result {
                        CoreValidationResult::Valid => "Valid".to_string(),
                        CoreValidationResult::Invalid(msg) => format!("Invalid({})", msg),
                    };
                    divergences.push((height, blvm_str.clone(), core_str.clone()));
                    eprintln!(
                        "❌ DIVERGENCE at height {}: BLVM={}, Core={}",
                        height, blvm_str, core_str
                    );

                    // Log first few divergences with more detail
                    if divergences.len() <= 5 {
                        use sha2::{Digest, Sha256};
                        if block_bytes.len() >= 80 {
                            let header = &block_bytes[0..80];
                            let first_hash = Sha256::digest(header);
                            let second_hash = Sha256::digest(&first_hash);
                            let mut hash_bytes = second_hash.as_slice().to_vec();
                            hash_bytes.reverse();
                            let block_hash = hex::encode(&hash_bytes[..8]);
                            eprintln!("   Block hash (first 8 bytes): {}", block_hash);
                        }
                    }
                } else {
                    matched += 1;
                }

                tested += 1;

                // Progress indicator every 100 blocks (more frequent for better feedback)
                if tested % 100 == 0 || tested == 1 {
                    let total = actual_end - chunk.start_height + 1;
                    let pct = 100.0 * tested as f64 / total as f64;
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let rate = tested as f64 / elapsed;
                    println!(
                        "📊 Chunk [{}-{}]: {}/{} blocks ({:.1}%) @ {:.1} blocks/sec",
                        chunk.start_height, actual_end, tested, total, pct, rate
                    );
                }
            }
        }
    }

    let duration = start_time.elapsed().as_secs_f64();

    Ok(ChunkResult {
        start_height: chunk.start_height,
        end_height: actual_end,
        tested,
        matched,
        divergences,
        duration_secs: duration,
    })
}

/// Run parallel differential tests
///
/// Uses optimized block data source (direct file reading if available, then cache, then RPC).
pub async fn run_parallel_differential(
    start_height: u64,
    end_height: u64,
    config: ParallelConfig,
    block_source: Arc<BlockDataSource>,
) -> Result<Vec<ChunkResult>> {
    // Get chain height
    let chain_height = match block_source.as_ref() {
        BlockDataSource::Rpc(client) => client.getblockcount().await?,
        BlockDataSource::RemoteCoreRpc(client) => client.get_block_count().await?,
        BlockDataSource::SharedCache(_, Some(client)) => client.getblockcount().await?,
        BlockDataSource::DirectFile(_) => {
            // For direct file reading, we don't know exact height
            // Use end_height as estimate
            end_height
        }
        _ => end_height,
    };
    let actual_end = end_height.min(chain_height);

    println!("🚀 Starting parallel differential test");
    println!("   Range: {} to {}", start_height, actual_end);
    println!("   Chunk size: {}", config.chunk_size);
    println!("   Workers: {}", config.num_workers);
    println!("   Use checkpoints: {}", config.use_checkpoints);

    // If index is incomplete, use RPC to fill missing blocks
    // Chunks are primary - RPC is fallback for any missing blocks
    if let Ok(cache_dir_str) = std::env::var("BLOCK_CACHE_DIR") {
        let cache_dir = std::path::Path::new(&cache_dir_str);
        use crate::chunk_index::load_block_index;
        if let Ok(Some(index)) = load_block_index(cache_dir) {
            // Check if index is incomplete (has gaps or missing entries)
            let expected_entries = (actual_end - start_height + 1) as usize;
            let has_gaps = index.len() < expected_entries;
            let missing_early_blocks = !index.contains_key(&start_height)
                || (start_height < 100 && !index.contains_key(&(start_height + 1)));

            if index.len() <= 1 || has_gaps || missing_early_blocks {
                // Index is incomplete - use RPC to fill missing blocks
                println!(
                    "\n🔨 Index incomplete ({} entries, expected {}) - filling missing blocks via remote-Core RPC...",
                    index.len(),
                    expected_entries
                );
                println!("   💡 Chunks are primary - RPC only fills gaps");

                use crate::chunk_index_rpc::build_block_index_via_rpc;
                let rpc_index = build_block_index_via_rpc(cache_dir, Some(actual_end)).await?;
                println!(
                    "   ✅ Built complete index via RPC ({} entries)",
                    rpc_index.len()
                );
                use crate::chunk_index::save_block_index;
                save_block_index(cache_dir, &rpc_index)?;
                println!("   💾 Saved complete index");
            }
        } else {
            // No index exists - build via RPC (chunks + missing blocks)
            println!(
                "\n🔨 No index found - building via remote-Core RPC (chunks + missing blocks)..."
            );
            use crate::chunk_index_rpc::build_block_index_via_rpc;
            let rpc_index = build_block_index_via_rpc(cache_dir, Some(actual_end)).await?;
            println!(
                "   ✅ Built complete index via RPC ({} entries)",
                rpc_index.len()
            );
            use crate::chunk_index::save_block_index;
            save_block_index(cache_dir, &rpc_index)?;
            println!("   💾 Saved complete index");
        }
    }

    // Generate checkpoints if enabled
    let checkpoints = if config.use_checkpoints {
        println!("\n📌 Phase 1: Generating UTXO checkpoints...");
        generate_checkpoints(
            start_height,
            actual_end,
            config.chunk_size,
            block_source.as_ref(),
        )
        .await?
    } else {
        Vec::new()
    };

    // Create chunks
    let mut chunks = Vec::new();
    let mut current_start = start_height;
    let mut checkpoint_idx = 0;

    while current_start <= actual_end {
        let chunk_end = (current_start + config.chunk_size - 1).min(actual_end);

        // Find checkpoint UTXO for this chunk
        let checkpoint_utxo = if config.use_checkpoints && checkpoint_idx > 0 {
            // Use previous checkpoint as starting UTXO
            checkpoints
                .get(checkpoint_idx - 1)
                .map(|(_, utxo)| utxo.clone())
        } else if current_start == start_height {
            // First chunk starts with empty UTXO set
            Some(UtxoSet::default())
        } else {
            None
        };

        chunks.push(BlockChunk {
            start_height: current_start,
            end_height: chunk_end,
            checkpoint_utxo,
            skip_validation: !config.use_checkpoints, // Skip validation if checkpoints disabled
        });

        current_start = chunk_end + 1;
        if current_start <= actual_end && checkpoint_idx < checkpoints.len() {
            checkpoint_idx += 1;
        }
    }

    println!(
        "\n📦 Created {} chunks for parallel execution",
        chunks.len()
    );

    // If checkpoints disabled, run sequential validation (no parallel chunks, but still validate!)
    if !config.use_checkpoints {
        println!(
            "\n🔍 Sequential validation mode (no checkpoints - validating blocks sequentially)..."
        );
        println!(
            "   This will validate blocks with both BLVM and Core, but sequentially (slower but works)"
        );

        // Create a single chunk for sequential validation
        let single_chunk = BlockChunk {
            start_height,
            end_height: actual_end,
            checkpoint_utxo: None, // No checkpoint - will validate from genesis
            skip_validation: false, // IMPORTANT: Actually validate!
        };

        println!("   🚀 Starting sequential differential validation...");
        println!(
            "   📊 Range: {} to {} ({} blocks)",
            start_height,
            actual_end,
            actual_end - start_height + 1
        );

        // Validate the single chunk sequentially
        let result = validate_chunk(single_chunk, block_source.clone()).await?;

        println!("   ✅ Sequential validation complete!");
        println!(
            "   📊 Results: {} tested, {} matched, {} divergences",
            result.tested,
            result.matched,
            result.divergences.len()
        );

        if result.divergences.len() > 0 {
            println!("   ⚠️  Found {} divergences:", result.divergences.len());
            for (height, blvm_result, core_result) in result.divergences.iter().take(10) {
                println!(
                    "      Height {}: BLVM={}, Core={}",
                    height, blvm_result, core_result
                );
            }
        } else {
            println!("   ✅ All blocks matched between BLVM and Core!");
        }

        return Ok(vec![result]);
    }

    // Run chunks in parallel with semaphore to limit concurrency
    let semaphore = Arc::new(Semaphore::new(config.num_workers));
    let mut handles = Vec::new();

    for chunk in chunks {
        let permit = semaphore.clone().acquire_owned().await?;
        let block_source_clone = block_source.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit;
            let result = validate_chunk(chunk, block_source_clone).await;
            result
        });

        handles.push(handle);
    }

    // Collect results
    println!("\n⚡ Phase 2: Running chunks in parallel...");
    let mut results = Vec::new();
    for (idx, handle) in handles.into_iter().enumerate() {
        match handle.await {
            Ok(Ok(result)) => {
                println!(
                    "✅ Chunk {} [{}-{}]: {} blocks, {} divergences, {:.1}s",
                    idx + 1,
                    result.start_height,
                    result.end_height,
                    result.tested,
                    result.divergences.len(),
                    result.duration_secs
                );
                results.push(result);
            }
            Ok(Err(e)) => {
                eprintln!("❌ Chunk {} failed: {}", idx + 1, e);
            }
            Err(e) => {
                eprintln!("❌ Chunk {} panicked: {}", idx + 1, e);
            }
        }
    }

    // Summary
    let total_tested: usize = results.iter().map(|r| r.tested).sum();
    let total_matched: usize = results.iter().map(|r| r.matched).sum();
    let total_divergences: usize = results.iter().map(|r| r.divergences.len()).sum();
    let total_duration: f64 = results.iter().map(|r| r.duration_secs).sum();

    println!("\n📊 Parallel Differential Test Summary:");
    println!("   Total blocks tested: {}", total_tested);
    println!("   Matched: {}", total_matched);
    println!("   Divergences: {}", total_divergences);
    println!(
        "   Total duration: {:.1}s ({:.1} minutes)",
        total_duration,
        total_duration / 60.0
    );
    println!(
        "   Throughput: {:.1} blocks/sec",
        total_tested as f64 / total_duration
    );

    if total_divergences > 0 {
        println!("\n❌ Divergences found:");
        for result in &results {
            for (height, blvm, core) in &result.divergences {
                println!("   Height {}: BLVM={}, Core={}", height, blvm, core);
            }
        }
    }

    Ok(results)
}
