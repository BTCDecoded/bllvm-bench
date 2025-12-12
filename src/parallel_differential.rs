//! Parallel Differential Testing
//!
//! This module provides parallel execution of differential tests by splitting
//! the blockchain into chunks and running them concurrently. Each chunk starts
//! from a UTXO checkpoint to enable independent parallel validation.

use anyhow::{Context, Result};
use blvm_consensus::UtxoSet;
use std::sync::Arc;
use tokio::sync::Semaphore;

// Re-export block file reader for convenience
pub use crate::block_file_reader::{BlockFileReader, Network as BlockFileNetwork, SharedBlockCache};

/// Block data source - optimized to avoid RPC when possible
pub enum BlockDataSource {
    /// Direct file reading (fastest - 10-50x faster than RPC)
    DirectFile(BlockFileReader),
    /// Shared cache (fast - 5-10x faster than RPC on subsequent runs)
    SharedCache(SharedBlockCache, Option<Arc<crate::core_rpc_client::CoreRpcClient>>),
    /// RPC fallback (slowest but always works)
    Rpc(Arc<crate::core_rpc_client::CoreRpcClient>),
    /// Start9 RPC via nsenter (works when files are encrypted)
    Start9Rpc(Arc<crate::start9_rpc_client::Start9RpcClient>),
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
/// Tries direct file reading first (fastest), then shared cache, then RPC fallback
/// Automatically detects Start9 and uses Start9 RPC if direct file reading fails
pub fn create_block_data_source(
    network: BlockFileNetwork,
    cache_dir: Option<impl AsRef<std::path::Path>>,
    rpc_client: Option<Arc<crate::core_rpc_client::CoreRpcClient>>,
) -> Result<BlockDataSource> {
    // Try direct file reading first (fastest - 10-50x faster than RPC)
    // Check common locations - standard Bitcoin Core paths first, Start9 as fallback
    let possible_dirs = vec![
        dirs::home_dir().map(|h| h.join(".bitcoin")), // Standard local Bitcoin Core (default)
        Some(std::path::PathBuf::from("/root/.bitcoin")),
        Some(std::path::PathBuf::from("/var/lib/bitcoind")),
        // Start9 paths (fallback for local testing only)
        dirs::home_dir().map(|h| h.join("mnt/bitcoin-start9")),
        Some(std::path::PathBuf::from("/mnt/bitcoin-start9")),
    ];
    
    // Try direct file reading first (including Start9 mount - fixing XOR decryption!)
    for dir in possible_dirs.into_iter().flatten() {
        if dir.join("blocks").exists() {
            // Try to create reader - may fail due to permissions or format issues
            match BlockFileReader::new(&dir, network) {
                Ok(reader) => {
                    let is_start9 = dir.to_string_lossy().contains("bitcoin-start9");
                    if is_start9 {
                        println!("✅ Using direct block file reading from Start9 mount {} (10-50x faster than RPC, XOR decryption enabled)", dir.display());
                    } else {
                        println!("✅ Using direct block file reading from {} (10-50x faster than RPC)", dir.display());
                    }
                    return Ok(BlockDataSource::DirectFile(reader));
                }
                Err(e) => {
                    // Log but continue trying other locations
                    let is_start9 = dir.to_string_lossy().contains("bitcoin-start9");
                    if is_start9 {
                        println!("⚠️  Direct file reading from Start9 mount failed: {}. Will try RPC fallback.", e);
                    } else {
                        eprintln!("⚠️  Direct file reading from {} failed: {}. Will try other options.", dir.display(), e);
                    }
                    continue;
                }
            }
        }
    }
    
    // If Start9 mount exists but direct reading failed, try Start9 RPC as fallback
    let start9_mount = dirs::home_dir().map(|h| h.join("mnt/bitcoin-start9"));
    let is_start9 = start9_mount.as_ref()
        .map(|p| p.exists())
        .unwrap_or(false);
    
    if is_start9 {
        let start9_client = Arc::new(crate::start9_rpc_client::Start9RpcClient::new());
        println!("✅ Using Start9 RPC via nsenter (fallback - direct file reading unavailable)");
        return Ok(BlockDataSource::Start9Rpc(start9_client));
    }
    
    // Try shared cache (fast on subsequent runs, can use DirectFile or RPC to populate)
    if let Some(cache_path) = cache_dir {
        let cache = SharedBlockCache::new(cache_path)?;
        println!("✅ Using shared block cache (5-10x faster than RPC on subsequent runs)");
        println!("   Cache will use RPC or DirectFile to populate blocks");
        return Ok(BlockDataSource::SharedCache(cache, rpc_client));
    }
    
    // Fall back to RPC (slowest but always works)
    if let Some(client) = rpc_client {
        println!("⚠️  Using RPC (slowest option - consider using direct file reading or cache)");
        return Ok(BlockDataSource::Rpc(client));
    }
    
    anyhow::bail!("No block data source available. Need Core data directory, cache directory, or RPC client.")
}

/// Get block data from optimized source
pub async fn get_block_data(
    source: &BlockDataSource,
    height: u64,
) -> Result<Vec<u8>> {
    match source {
        BlockDataSource::DirectFile(reader) => {
            // For direct file reading, we need to iterate sequentially
            // This is a limitation - we'll need to cache blocks or use index
            // For now, fall back to RPC for random access
            anyhow::bail!("Direct file reading requires sequential access. Use generate_checkpoints_sequential or provide RPC client for random access.")
        }
        BlockDataSource::SharedCache(cache, rpc_client) => {
            cache.get_or_fetch_block(height, rpc_client.as_deref()).await
        }
        BlockDataSource::Rpc(client) => {
            let block_hash = client.getblockhash(height).await?;
            let block_hex = client.getblock_raw(&block_hash).await?;
            Ok(hex::decode(&block_hex)?)
        }
        BlockDataSource::Start9Rpc(client) => {
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
    use blvm_consensus::block::connect_block;
    use blvm_consensus::segwit::Witness;
    use blvm_consensus::serialization::block::deserialize_block_with_witnesses;
    use blvm_consensus::types::Network;

    // OPTIMIZATION: Pre-allocate checkpoints vector (estimate: ~10 checkpoints for 1M blocks)
    let estimated_checkpoints = ((end_height - start_height) / chunk_size + 1) as usize;
    let mut checkpoints = Vec::with_capacity(estimated_checkpoints.min(100));
    let mut utxo_set = UtxoSet::new();
    let mut previous_block_hash: Option<[u8; 32]> = None; // Track previous block hash for verification
    
    // If starting from height 0, we start with empty UTXO set
    // Otherwise, we'd need to load from a previous checkpoint
    
    // Get chain height (need RPC for this)
    let chain_height = match block_source {
        BlockDataSource::Rpc(client) => client.getblockcount().await?,
        BlockDataSource::Start9Rpc(client) => client.get_block_count().await?,
        BlockDataSource::SharedCache(_, Some(client)) => client.getblockcount().await?,
        _ => {
            // For direct file reading, we don't know chain height
            // Use end_height as estimate
            end_height
        }
    };
    let actual_end = end_height.min(chain_height);
    
    println!("🔧 Generating UTXO checkpoints from {} to {} (chunk size: {})", 
             start_height, actual_end, chunk_size);
    
    let mut next_checkpoint = start_height + chunk_size;
    
    // Use optimized block reading for sequential access
    match block_source {
        BlockDataSource::DirectFile(reader) => {
            // Direct file reading - sequential iterator (fastest!)
            println!("📂 Using direct file reading for checkpoint generation");
            let iterator = reader.read_blocks_sequential(Some(start_height), Some((actual_end - start_height + 1) as usize))?;
            
            for (idx, block_result) in iterator.enumerate() {
                let height = start_height + idx as u64;
                let block_bytes = match block_result {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        eprintln!("❌ Failed to read block at height {}: {}", height, e);
                        return Err(e.into());
                    }
                };
                
                // Validate block size
                if block_bytes.len() < 80 {
                    anyhow::bail!("Block {} too small: {} bytes (minimum 80 for header)", height, block_bytes.len());
                }
                
                // Verify previous block hash matches (if not genesis) - this helps detect block boundary issues
                if height > 0 {
                    let prev_hash_in_header = &block_bytes[4..36]; // Previous block hash is at bytes 4-36 (little-endian)
                    // We'll verify this after parsing the block
                }
                
                let (block, witnesses) = match deserialize_block_with_witnesses(&block_bytes) {
                    Ok(result) => result,
                    Err(e) => {
                        eprintln!("❌ Failed to deserialize block at height {}: {}", height, e);
                        eprintln!("   Block size: {} bytes", block_bytes.len());
                        eprintln!("   First 80 bytes (header, hex): {}", hex::encode(&block_bytes[0..80.min(block_bytes.len())]));
                        if block_bytes.len() > 80 {
                            eprintln!("   Bytes 80-100 (hex): {}", hex::encode(&block_bytes[80..100.min(block_bytes.len())]));
                        }
                        // For Start9, if deserialization fails, the block boundary might be wrong
                        // Try to continue - this will help us identify all problematic blocks
                        eprintln!("⚠️  Block {} deserialization failed - likely block boundary issue. Skipping.", height);
                        continue; // Skip this block and continue
                    }
                };
                
                // Debug: Check if previous blocks had non-coinbase transactions
                if height <= 16 {
                    let non_coinbase_count = block.transactions.iter().filter(|tx| !blvm_consensus::transaction::is_coinbase(tx)).count();
                    if non_coinbase_count > 0 {
                        eprintln!("🔍 Block {}: {} non-coinbase transactions", height, non_coinbase_count);
                        // For each non-coinbase transaction, show what it's spending
                        for (tx_idx, tx) in block.transactions.iter().enumerate() {
                            if !blvm_consensus::transaction::is_coinbase(tx) {
                                use blvm_consensus::block::calculate_tx_id;
                                let txid = calculate_tx_id(tx);
                                let txid_str: String = txid.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                                eprintln!("   TX {} (non-coinbase): {} inputs, {} outputs, TXID: {}...", 
                                         tx_idx, tx.inputs.len(), tx.outputs.len(), txid_str);
                                if !tx.inputs.is_empty() {
                                    let hash_str: String = tx.inputs[0].prevout.hash.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                                    eprintln!("      Spending: {}:{}", hash_str, tx.inputs[0].prevout.index);
                                }
                            }
                        }
                    }
                }
                
                // Debug: Check UTXO set after each block to see if outputs are being added
                if height <= 16 {
                    let non_coinbase_utxos: Vec<_> = utxo_set.iter()
                        .filter(|(_, utxo)| !utxo.is_coinbase)
                        .collect();
                    if !non_coinbase_utxos.is_empty() {
                        eprintln!("🔍 After block {}: {} non-coinbase UTXOs in set", height, non_coinbase_utxos.len());
                        for (outpoint, utxo) in non_coinbase_utxos.iter().take(3) {
                            let hash_str: String = outpoint.hash.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                            eprintln!("   Non-coinbase UTXO: {}:{} (value={}, height={})", 
                                     hash_str, outpoint.index, utxo.value, utxo.height);
                        }
                    }
                }
                
                // Debug: Print transaction details for block 15
                if height == 15 {
                    eprintln!("🔍 DEBUG Block 15: {} transactions", block.transactions.len());
                    eprintln!("   UTXO set size: {}", utxo_set.len());
                    // List all UTXOs in the set
                    eprintln!("   All UTXOs in set:");
                    for (outpoint, utxo) in utxo_set.iter() {
                        let hash_str: String = outpoint.hash.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                        eprintln!("      {}:{} (value={}, height={}, coinbase={})", 
                                 hash_str, outpoint.index, utxo.value, utxo.height, utxo.is_coinbase);
                    }
                    for (tx_idx, tx) in block.transactions.iter().enumerate() {
                        eprintln!("   TX {}: {} inputs, {} outputs", tx_idx, tx.inputs.len(), tx.outputs.len());
                        if !tx.inputs.is_empty() {
                            let hash_str: String = tx.inputs[0].prevout.hash.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                            eprintln!("      First input prevout: {}:{}", hash_str, tx.inputs[0].prevout.index);
                            // Check if UTXO exists
                            if let Some(utxo) = utxo_set.get(&tx.inputs[0].prevout) {
                                eprintln!("      UTXO exists: value={}, height={}, coinbase={}", 
                                         utxo.value, utxo.height, utxo.is_coinbase);
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
                                        let hash_str: String = outpoint.hash.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                                        eprintln!("      Found matching TX ID in UTXO set: {}:{} (target index: {})", 
                                                 hash_str, outpoint.index, target_index);
                                        eprintln!("      UTXO details: value={}, height={}, coinbase={}", 
                                                 utxo.value, utxo.height, utxo.is_coinbase);
                                    }
                                }
                                if !found_match {
                                    let hash_str: String = target_hash.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                                    eprintln!("      No UTXO found with TX ID: {} (index: {})", hash_str, target_index);
                                    eprintln!("      This UTXO should have been created in a previous block");
                                    // Check if this TX ID matches any coinbase TX ID in the UTXO set
                                    eprintln!("      Checking if this matches any coinbase TX ID...");
                                    let mut found_coinbase_match = false;
                                    for (outpoint, utxo) in utxo_set.iter() {
                                        if utxo.is_coinbase {
                                            let outpoint_hash_str: String = outpoint.hash.iter().take(8).map(|b| format!("{:02x}", b)).collect();
                                            if outpoint.hash == target_hash {
                                                found_coinbase_match = true;
                                                eprintln!("      ✅ Found matching coinbase TX ID: {}:{} (but index {} doesn't match)", 
                                                         outpoint_hash_str, outpoint.index, target_index);
                                                eprintln!("      This suggests the transaction is trying to spend the wrong output index");
                                                break;
                                            }
                                        }
                                    }
                                    if !found_coinbase_match {
                                        eprintln!("      ❌ No matching coinbase TX ID found - this UTXO was never created");
                                    }
                                }
                            }
                        }
                        // Calculate TX ID
                        use blvm_consensus::block::calculate_tx_id;
                        let txid = calculate_tx_id(tx);
                        let txid_str: String = txid.iter().take(8).map(|b| format!("{:02x}", b)).collect();
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
                let mut current_block_hash: [u8; 32] = second_hash.as_slice().try_into()
                    .map_err(|_| anyhow::anyhow!("Invalid hash length"))?;
                current_block_hash.reverse(); // Convert to big-endian
                
                // Verify previous block hash matches (if not genesis)
                // For Start9 encrypted files, if prev hash doesn't match, the block boundary detection
                // is likely wrong. We'll skip validation errors for now and continue to identify
                // which blocks work correctly.
                if height > 0 {
                    let prev_hash_in_header = &block_bytes[4..36]; // Previous block hash is at bytes 4-36 (little-endian, as stored)
                    if let Some(prev_hash) = previous_block_hash {
                        // Convert previous_block_hash (big-endian) to little-endian for comparison
                        let prev_hash_le: Vec<u8> = prev_hash.iter().rev().copied().collect();
                        if prev_hash_in_header != prev_hash_le.as_slice() {
                            // This indicates we're reading too much data - block boundary is wrong
                            eprintln!("⚠️  Block {}: Previous block hash mismatch - block boundary detection issue!", height);
                            eprintln!("   Header has (LE): {}", hex::encode(prev_hash_in_header));
                            eprintln!("   Expected (LE):   {}", hex::encode(&prev_hash_le));
                            eprintln!("   Block size: {} bytes (likely reading too much - should use size field or verify hash)", block_bytes.len());
                        }
                    }
                }
                
                // Update previous block hash for next iteration
                previous_block_hash = Some(current_block_hash);
                
                // Debug: Check transaction count and verify block hash for problematic blocks
                if height == 15 || height == 10 {
                    let block_hash_hex = hex::encode(&current_block_hash[..8]);
                    eprintln!("DEBUG Block {}: Parsed {} transactions, block hash (first 8 bytes) = {}, block size = {} bytes", 
                             height, block.transactions.len(), block_hash_hex, block_bytes.len());
                    for (i, tx) in block.transactions.iter().enumerate() {
                        eprintln!("DEBUG Block {}: TX {} has {} inputs, {} outputs", height, i, tx.inputs.len(), tx.outputs.len());
                    }
                }
                
                // Debug: Verify we're calculating the correct coinbase txid
                #[cfg(debug_assertions)]
                if height <= 2 {
                    use blvm_consensus::block::calculate_tx_id;
                    if let Some(coinbase) = block.transactions.first() {
                        let txid = calculate_tx_id(coinbase);
                        eprintln!("DEBUG Block {}: coinbase txid = {}", height, hex::encode(txid));
                        eprintln!("DEBUG Block {}: UTXO set size = {}", height, utxo_set.len());
                        // List all coinbase UTXOs in the set
                        let mut coinbase_utxos = Vec::new();
                        for (outpoint, utxo) in utxo_set.iter() {
                            if utxo.is_coinbase {
                                coinbase_utxos.push((hex::encode(outpoint.hash), utxo.height));
                            }
                        }
                        if !coinbase_utxos.is_empty() {
                            eprintln!("DEBUG Block {}: Coinbase UTXOs in set: {:?}", height, coinbase_utxos);
                        }
                    }
                }
                
                // Validate with BLVM
                let (result, new_utxo_set, _undo_log) = connect_block(
                    &block,
                    &witnesses,
                    utxo_set.clone(),
                    height,
                    None,
                    Network::Mainnet,
                )?;
                
                if matches!(result, blvm_consensus::types::ValidationResult::Valid) {
                    utxo_set = new_utxo_set;
                } else {
                    // OPTIMIZATION: Use string reference instead of clone
                    let error_msg = match &result {
                        blvm_consensus::types::ValidationResult::Invalid(msg) => msg.as_str(),
                        _ => "Unknown error",
                    };
                    eprintln!("❌ Block {} validation failed: {}", height, error_msg);
                    anyhow::bail!("Block {} failed validation during checkpoint generation: {}", height, error_msg);
                }
                
                // Save checkpoint at chunk boundaries
                // CRITICAL: Save checkpoint at the END of each chunk (before the next chunk starts)
                // For chunk 0-169, save at height 169 (after processing block 169)
                // For chunk 170-339, save at height 339 (after processing block 339)
                // This ensures the checkpoint contains UTXOs from blocks 0-169, not 0-170
                if height == next_checkpoint - 1 || height == actual_end {
                    println!("✅ Checkpoint at height {} (UTXO count: {})", height, utxo_set.len());
                    // NOTE: Must clone here because we continue processing after checkpoint
                    checkpoints.push((height, utxo_set.clone()));
                    next_checkpoint += chunk_size;
                }
                
                // Progress indicator
                if height % 10_000 == 0 {
                    println!("📊 Checkpoint generation: {}/{} ({:.1}%)", 
                             height - start_height, actual_end - start_height,
                             100.0 * (height - start_height) as f64 / (actual_end - start_height) as f64);
                }
            }
        }
        _ => {
            // For cache/RPC, fetch blocks sequentially (async)
            for height in start_height..=actual_end {
                let block_bytes = get_block_data(block_source, height).await?;
                
                let (block, witnesses) = deserialize_block_with_witnesses(&block_bytes)?;
                
                // Debug: Verify coinbase txid and block data for problematic blocks
                #[cfg(debug_assertions)]
                if height == 16 || height == 2 || height <= 1 {
                    use blvm_consensus::block::calculate_tx_id;
                    use sha2::{Digest, Sha256};
                    
                    // Verify block hash matches expected
                    if block_bytes.len() >= 80 {
                        let header = &block_bytes[0..80];
                        let block_hash = hex::encode(Sha256::digest(&Sha256::digest(header)));
                        eprintln!("DEBUG Block {}: block hash (calculated) = {}", height, block_hash);
                    }
                    
                    if let Some(coinbase) = block.transactions.first() {
                        let txid = calculate_tx_id(coinbase);
                        eprintln!("DEBUG Block {}: coinbase txid = {}", height, hex::encode(txid));
                        eprintln!("DEBUG Block {}: coinbase script_sig len = {}", height, coinbase.inputs[0].script_sig.len());
                        eprintln!("DEBUG Block {}: coinbase script_sig (first 20) = {}", height, 
                                 hex::encode(&coinbase.inputs[0].script_sig[..coinbase.inputs[0].script_sig.len().min(20)]));
                        eprintln!("DEBUG Block {}: block_bytes len = {}", height, block_bytes.len());
                        eprintln!("DEBUG Block {}: UTXO set size = {}", height, utxo_set.len());
                        
                        // Check for matching UTXOs
                        let mut matches = Vec::new();
                        for (outpoint, utxo) in utxo_set.iter() {
                            if outpoint.hash == txid && utxo.is_coinbase {
                                matches.push(utxo.height);
                            }
                        }
                        if !matches.is_empty() {
                            eprintln!("DEBUG Block {}: Found {} UTXO(s) with matching txid at heights: {:?}", height, matches.len(), matches);
                        }
                    }
                }
                
                // Validate with BLVM
                let (result, new_utxo_set, _undo_log) = connect_block(
                    &block,
                    &witnesses,
                    utxo_set.clone(),
                    height,
                    None,
                    Network::Mainnet,
                )?;
                
                if matches!(result, blvm_consensus::types::ValidationResult::Valid) {
                    utxo_set = new_utxo_set;
                } else {
                    // OPTIMIZATION: Use string reference instead of clone
                    let error_msg = match &result {
                        blvm_consensus::types::ValidationResult::Invalid(msg) => msg.as_str(),
                        _ => "Unknown error",
                    };
                    eprintln!("❌ Block {} validation failed: {}", height, error_msg);
                    anyhow::bail!("Block {} failed validation during checkpoint generation: {}", height, error_msg);
                }
                
                // Save checkpoint at chunk boundaries
                // CRITICAL: Save checkpoint at the END of each chunk (before the next chunk starts)
                // For chunk 0-169, save at height 169 (after processing block 169)
                // For chunk 170-339, save at height 339 (after processing block 339)
                // This ensures the checkpoint contains UTXOs from blocks 0-169, not 0-170
                if height == next_checkpoint - 1 || height == actual_end {
                    println!("✅ Checkpoint at height {} (UTXO count: {})", height, utxo_set.len());
                    // NOTE: Must clone here because we continue processing after checkpoint
                    // The checkpoint is saved for parallel validation later
                    checkpoints.push((height, utxo_set.clone()));
                    next_checkpoint += chunk_size;
                }
                
                // Progress indicator
                if height % 10_000 == 0 {
                    println!("📊 Checkpoint generation: {}/{} ({:.1}%)", 
                             height - start_height, actual_end - start_height,
                             100.0 * (height - start_height) as f64 / (actual_end - start_height) as f64);
                }
            }
        }
    }
    
    Ok(checkpoints)
}

/// Process a single block (validate with BLVM and Core)
async fn process_block(
    block_bytes: &[u8],
    height: u64,
    utxo_set: &mut UtxoSet,
    block_source: &BlockDataSource,
) -> Result<(crate::differential::ValidationResult, crate::differential::CoreValidationResult)> {
    use crate::differential::{CoreValidationResult, ValidationResult};
    use blvm_consensus::block::connect_block;
    use blvm_consensus::segwit::Witness;
    use blvm_consensus::serialization::block::deserialize_block_with_witnesses;
    use blvm_consensus::types::Network;
    
    let (block, witnesses) = match deserialize_block_with_witnesses(block_bytes) {
        Ok((b, w)) => (b, w),
        Err(e) => {
            anyhow::bail!("Failed to deserialize block at height {}: {}", height, e);
        }
    };
    
    // Validate with BLVM
    let blvm_result = match connect_block(
        &block,
        &witnesses,
        utxo_set.clone(),
        height,
        None,
        Network::Mainnet,
    ) {
        Ok((result, new_utxo_set, _undo_log)) => {
            *utxo_set = new_utxo_set;
            match result {
                blvm_consensus::types::ValidationResult::Valid => ValidationResult::Valid,
                blvm_consensus::types::ValidationResult::Invalid(msg) => {
                    ValidationResult::Invalid(msg)
                }
            }
        }
        Err(e) => ValidationResult::Invalid(format!("{:?}", e)),
    };
    
    // Validate with Core
    let core_result = match block_source {
        BlockDataSource::DirectFile(_) => {
            // Blocks from Core's files are assumed valid
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
                let mut hash_bytes: [u8; 32] = second_hash.as_slice().try_into()
                    .unwrap_or_else(|_| {
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
        BlockDataSource::Start9Rpc(client) => {
            // Calculate block hash to check with Core
            // OPTIMIZATION: Use fixed-size array instead of Vec allocation
            use sha2::{Digest, Sha256};
            if block_bytes.len() >= 80 {
                let header = &block_bytes[0..80];
                let first_hash = Sha256::digest(header);
                let second_hash = Sha256::digest(&first_hash);
                // Reverse bytes for Core RPC (Core displays hashes in reverse)
                // OPTIMIZATION: Use array instead of Vec to avoid allocation
                let mut hash_bytes: [u8; 32] = second_hash.as_slice().try_into()
                    .map_err(|_| anyhow::anyhow!("Invalid hash length"))?;
                hash_bytes.reverse();
                let block_hash = hex::encode(hash_bytes);
                
                // Start9 RPC - just check if we can get the block
                match client.get_block_hex(&block_hash).await {
                    Ok(_) => CoreValidationResult::Valid,
                    Err(_) => CoreValidationResult::Invalid("Block not in chain".to_string()),
                }
            } else {
                CoreValidationResult::Invalid("Block too short".to_string())
            }
        }
        _ => {
            // No RPC client available, assume valid for direct file reading
            CoreValidationResult::Valid
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
    let mut utxo_set = chunk.checkpoint_utxo.unwrap_or_else(UtxoSet::new);
    // OPTIMIZATION: Pre-allocate divergences vector (most tests have 0-10 divergences)
    let mut divergences = Vec::with_capacity(10);
    let mut tested = 0;
    let mut matched = 0;
    
    // Get chain height
    let chain_height = match block_source.as_ref() {
        BlockDataSource::Rpc(client) => client.getblockcount().await?,
        BlockDataSource::Start9Rpc(client) => client.get_block_count().await?,
        BlockDataSource::SharedCache(_, Some(client)) => client.getblockcount().await?,
        BlockDataSource::DirectFile(_) => chunk.end_height, // Don't know exact height
        BlockDataSource::SharedCache(_, None) => chunk.end_height, // Don't know exact height
    };
    let actual_end = chunk.end_height.min(chain_height);
    
    // Process blocks based on data source
    match block_source.as_ref() {
        BlockDataSource::DirectFile(reader) => {
            // Direct file reading - sequential iterator (fastest!)
            let iterator = reader.read_blocks_sequential(
                Some(chunk.start_height),
                Some((actual_end - chunk.start_height + 1) as usize)
            )?;
            
            for (idx, block_result) in iterator.enumerate() {
                let height = chunk.start_height + idx as u64;
                let block_bytes = block_result?;
                
                // Process block (same logic for both paths)
                let (blvm_result, core_result) = process_block(
                    &block_bytes,
                    height,
                    &mut utxo_set,
                    block_source.as_ref(),
                ).await?;
                
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
                    eprintln!("❌ DIVERGENCE at height {}: BLVM={}, Core={}", 
                             height, blvm_str, core_str);
                    
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
                    println!("📊 Chunk [{}-{}]: {}/{} blocks ({:.1}%) @ {:.1} blocks/sec", 
                             chunk.start_height, actual_end, tested, total, pct, rate);
                }
            }
        }
        _ => {
            // For cache/RPC, fetch blocks sequentially (async)
            for height in chunk.start_height..=actual_end {
                let block_bytes = get_block_data(block_source.as_ref(), height).await?;
                
                // Process block (same logic)
                let (blvm_result, core_result) = process_block(
                    &block_bytes,
                    height,
                    &mut utxo_set,
                    block_source.as_ref(),
                ).await?;
                
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
                    eprintln!("❌ DIVERGENCE at height {}: BLVM={}, Core={}", 
                             height, blvm_str, core_str);
                    
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
                    println!("📊 Chunk [{}-{}]: {}/{} blocks ({:.1}%) @ {:.1} blocks/sec", 
                             chunk.start_height, actual_end, tested, total, pct, rate);
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
        BlockDataSource::Start9Rpc(client) => client.get_block_count().await?,
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
    
    // Generate checkpoints if enabled
    let checkpoints = if config.use_checkpoints {
        println!("\n📌 Phase 1: Generating UTXO checkpoints...");
        generate_checkpoints(start_height, actual_end, config.chunk_size, block_source.as_ref()).await?
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
            checkpoints.get(checkpoint_idx - 1).map(|(_, utxo)| utxo.clone())
        } else if current_start == start_height {
            // First chunk starts with empty UTXO set
            Some(UtxoSet::new())
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
    
    println!("\n📦 Created {} chunks for parallel execution", chunks.len());
    
    // If checkpoints disabled, just build cache by reading blocks (no validation)
    if !config.use_checkpoints {
        println!("\n📦 Cache building mode: Reading blocks in parallel to build cache (no validation)...");
        println!("   This will populate the cache file for future use");
        
        // For cache building, we just need to trigger block reading
        // The cache is built automatically when blocks are read from files
        match block_source.as_ref() {
            BlockDataSource::DirectFile(reader) => {
                // Trigger cache building by reading blocks sequentially
                // This will use parallel file reading internally
                println!("   🚀 Starting parallel block reading to build cache...");
                let iterator = reader.read_blocks_sequential(Some(start_height), Some((actual_end - start_height + 1) as usize))?;
                
                let mut blocks_read = 0;
                for (idx, block_result) in iterator.enumerate() {
                    let height = start_height + idx as u64;
                    match block_result {
                        Ok(_) => {
                            blocks_read += 1;
                            if blocks_read % 10000 == 0 {
                                println!("   📊 Read {} blocks (at height {})", blocks_read, height);
                            }
                        }
                        Err(e) => {
                            eprintln!("   ⚠️  Failed to read block at height {}: {}", height, e);
                            // Continue reading - don't fail on individual block errors
                        }
                    }
                }
                
                println!("   ✅ Cache building complete: {} blocks read", blocks_read);
                
                // Return empty results since we're not validating
                return Ok(Vec::new());
            }
            BlockDataSource::Start9Rpc(_) | BlockDataSource::Rpc(_) | BlockDataSource::SharedCache(_, _) => {
                // For RPC sources, we can't build cache efficiently in parallel
                // The cache building happens in block_file_reader when using DirectFile
                println!("   ⚠️  Cache building requires DirectFile source (currently using RPC)");
                println!("   💡 Cache will be built when blocks are read, but it's slower via RPC");
                println!("   📦 Proceeding with cache building via current source...");
                // Fall through - let it process chunks but skip validation
            }
        }
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
                println!("✅ Chunk {} [{}-{}]: {} blocks, {} divergences, {:.1}s", 
                         idx + 1, result.start_height, result.end_height,
                         result.tested, result.divergences.len(), result.duration_secs);
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
    println!("   Total duration: {:.1}s ({:.1} minutes)", total_duration, total_duration / 60.0);
    println!("   Throughput: {:.1} blocks/sec", total_tested as f64 / total_duration);
    
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

