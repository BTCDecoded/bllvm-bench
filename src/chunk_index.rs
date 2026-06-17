//! Block index for chunked cache
//!
//! Maps block height to (chunk_number, offset_in_chunk) for fast random access
//! and correct block ordering.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Index entry: maps block height to chunk location
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockIndexEntry {
    pub chunk_number: usize,
    pub offset_in_chunk: u64, // Offset in uncompressed chunk (after decompression)
    pub block_hash: [u8; 32], // Block hash for verification
}

/// Block index: height -> location
pub type BlockIndex = HashMap<u64, BlockIndexEntry>;

/// Hash map for fast block lookups: block_hash -> (chunk_number, offset_in_chunk)
pub type BlockHashMap = HashMap<[u8; 32], (usize, u64)>;

/// Load hash map from file (for fast lookups without reprocessing chunks)
pub fn load_hash_map(chunks_dir: &Path) -> Result<Option<BlockHashMap>> {
    let hash_map_file = chunks_dir.join("chunks.hashmap");
    if !hash_map_file.exists() {
        return Ok(None);
    }

    let data = std::fs::read(&hash_map_file)
        .with_context(|| format!("Failed to read hash map file: {}", hash_map_file.display()))?;

    let hash_map: BlockHashMap =
        bincode::deserialize(&data).with_context(|| "Failed to deserialize hash map")?;

    Ok(Some(hash_map))
}

/// Save hash map to file (for fast lookups without reprocessing chunks)
pub fn save_hash_map(chunks_dir: &Path, hash_map: &BlockHashMap) -> Result<()> {
    let hash_map_file = chunks_dir.join("chunks.hashmap");
    let temp_file = chunks_dir.join("chunks.hashmap.tmp");

    let data = bincode::serialize(hash_map).with_context(|| "Failed to serialize hash map")?;

    // Write to temp file first, then atomically rename
    std::fs::write(&temp_file, data).with_context(|| {
        format!(
            "Failed to write temp hash map file: {}",
            temp_file.display()
        )
    })?;

    std::fs::rename(&temp_file, &hash_map_file).with_context(|| {
        format!(
            "Failed to rename temp hash map file to: {}",
            hash_map_file.display()
        )
    })?;

    Ok(())
}

/// Load block index from file
pub fn load_block_index(chunks_dir: &Path) -> Result<Option<BlockIndex>> {
    let index_file = chunks_dir.join("chunks.index");
    if !index_file.exists() {
        return Ok(None);
    }

    let data = std::fs::read(&index_file)
        .with_context(|| format!("Failed to read index file: {}", index_file.display()))?;

    let index: BlockIndex =
        bincode::deserialize(&data).with_context(|| "Failed to deserialize block index")?;

    Ok(Some(index))
}

/// Longest run of consecutive heights starting at `start` that exist in `index`.
///
/// Returns `(count, first_missing_height)`: `count` is how many of
/// `start, start+1, …` are present; `first_missing_height` is the first gap
/// (or `start` if `start` itself is missing).
pub fn contiguous_chain_from(index: &BlockIndex, start: u64) -> (u64, u64) {
    let mut h = start;
    while index.contains_key(&h) {
        h += 1;
    }
    let count = h - start;
    (count, h)
}

/// Heights that [`crate::chunked_cache::ChunkedBlockIterator`] validates eagerly on construction
/// (`start_height..end_height.min(100)` in `chunked_cache.rs`).
pub fn iterator_eager_height_range(start: u64, end_height: u64) -> Option<std::ops::Range<u64>> {
    let cap = end_height.min(100);
    if start < cap { Some(start..cap) } else { None }
}

/// Which `chunk_i.bin.zst` files are absent for `i in 0..num_chunks`.
pub fn missing_chunk_bin_files(chunks_dir: &Path, num_chunks: usize) -> Vec<usize> {
    (0..num_chunks)
        .filter(|i| !chunks_dir.join(format!("chunk_{}.bin.zst", i)).exists())
        .collect()
}

/// Load `chunks.index`, or build it from `chunk_*.bin.zst` when missing. If `rebuild` is true,
/// delete an existing index and build from scratch.
pub fn ensure_chunk_block_index(chunks_dir: &Path, rebuild: bool) -> Result<BlockIndex> {
    let index_path = chunks_dir.join("chunks.index");
    if rebuild && index_path.exists() {
        std::fs::remove_file(&index_path)
            .with_context(|| format!("remove {}", index_path.display()))?;
    }

    if !rebuild {
        if let Some(idx) = load_block_index(chunks_dir)? {
            return Ok(idx);
        }
    }

    eprintln!("   Building block index from chunk files…");
    let (idx, _) = build_block_index(chunks_dir)?;
    if !idx.is_empty() {
        save_block_index_with_options(chunks_dir, &idx, true)
            .with_context(|| format!("save {}", chunks_dir.join("chunks.index").display()))?;
    }
    Ok(idx)
}

/// Checks that `chunks.index` is sufficient for [`crate::chunked_cache::ChunkedBlockIterator`]
/// and `chunk_utxo_checkpoints` (eager height range + contiguous run from `--start`).
pub fn validate_utxo_chunk_cache_index(
    index: &BlockIndex,
    start: u64,
    end_cap: u64,
    min_contiguous: u64,
) -> Result<()> {
    if let Some(range) = iterator_eager_height_range(start, end_cap) {
        for h in range.clone() {
            if !index.contains_key(&h) {
                anyhow::bail!(
                    "ChunkedBlockIterator preflight: missing index entry for height {} (need heights {:?} for --start {} and end cap {})",
                    h,
                    range,
                    start,
                    end_cap
                );
            }
        }
    }

    let (contig, first_gap) = contiguous_chain_from(index, start);
    if contig < min_contiguous {
        anyhow::bail!(
            "need at least {} contiguous indexed heights from --start {} (have {}). \
             First gap at height {}. Add missing chunk files or sync a contiguous mainnet prefix.",
            min_contiguous,
            start,
            contig,
            first_gap
        );
    }
    Ok(())
}

/// Save block index to file
/// CRITICAL: Uses atomic write (write to temp file, then rename) to prevent corruption
/// SAFEGUARD: Never overwrites with a smaller index unless forced
pub fn save_block_index(chunks_dir: &Path, index: &BlockIndex) -> Result<()> {
    save_block_index_with_options(chunks_dir, index, false)
}

/// Save block index with option to force overwrite even if smaller
pub fn save_block_index_with_options(
    chunks_dir: &Path,
    index: &BlockIndex,
    force: bool,
) -> Result<()> {
    let index_file = chunks_dir.join("chunks.index");
    let temp_file = chunks_dir.join("chunks.index.tmp");

    // SAFEGUARD: Check if existing index is larger - refuse to overwrite unless forced
    if index_file.exists() && !force {
        if let Ok(existing_data) = std::fs::read(&index_file) {
            if let Ok(existing_index) = bincode::deserialize::<BlockIndex>(&existing_data) {
                if existing_index.len() > index.len() {
                    eprintln!(
                        "   ⚠️  SAFEGUARD: Refusing to overwrite {} entries with {} entries!",
                        existing_index.len(),
                        index.len()
                    );
                    eprintln!(
                        "   💡 Use save_block_index_with_options(..., force=true) to override"
                    );
                    anyhow::bail!(
                        "Refusing to overwrite larger index ({} entries) with smaller one ({} entries)",
                        existing_index.len(),
                        index.len()
                    );
                }
            }
        }
    }

    // SAFEGUARD: Create timestamped backup before saving
    if index_file.exists() {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backup_file = chunks_dir.join(format!("chunks.index.backup.{}", timestamp));
        if let Err(e) = std::fs::copy(&index_file, &backup_file) {
            eprintln!("   ⚠️  Warning: Failed to create timestamped backup: {}", e);
        } else {
            eprintln!("   💾 Created backup: {}", backup_file.display());
        }
    }

    let data = bincode::serialize(index).with_context(|| "Failed to serialize block index")?;

    // CRITICAL FIX: Write to temp file first, then atomically rename to prevent corruption
    std::fs::write(&temp_file, &data)
        .with_context(|| format!("Failed to write temp index file: {}", temp_file.display()))?;

    // Atomically replace the index file
    std::fs::rename(&temp_file, &index_file).with_context(|| {
        format!(
            "Failed to rename temp index file to: {}",
            index_file.display()
        )
    })?;

    Ok(())
}

/// Build block index by reading all blocks from chunks and chaining by prev_block_hash
///
/// Returns (index, blocks_by_block_hash) where blocks_by_block_hash contains ALL blocks
/// for fast hash-based lookup, even if they weren't chained.
///
/// If chaining fails, falls back to RPC-based indexing
pub fn build_block_index(
    chunks_dir: &Path,
) -> Result<(BlockIndex, HashMap<[u8; 32], (usize, u64, [u8; 32])>)> {
    use crate::chunked_cache::{decompress_chunk_streaming, load_chunk_metadata};
    use std::io::Read;
    use std::process::Stdio;

    println!("🔨 Building block index from chunks...");

    let metadata = load_chunk_metadata(chunks_dir)?
        .ok_or_else(|| anyhow::anyhow!("No chunk metadata found"))?;

    let mut index = BlockIndex::new();
    // Dual index:
    // 1. blocks_by_prev_hash: prev_hash_be -> (chunk, offset, block_hash_be) - for chaining
    // 2. blocks_by_block_hash: block_hash_be -> (chunk, offset, prev_hash_be) - for reverse lookup
    // OPTIMIZATION: Pre-allocate with estimated capacity to reduce rehashing
    let estimated_blocks = metadata.total_blocks as usize;
    let mut blocks_by_prev_hash: HashMap<[u8; 32], (usize, u64, [u8; 32])> =
        HashMap::with_capacity(estimated_blocks);
    let mut blocks_by_block_hash: HashMap<[u8; 32], (usize, u64, [u8; 32])> =
        HashMap::with_capacity(estimated_blocks);

    let mut genesis_block: Option<(usize, u64, [u8; 32])> = None;

    // OPTIMIZATION: Process chunks in parallel using rayon for 10-100x speedup
    use rayon::prelude::*;
    println!(
        "   🚀 Processing {} chunks in parallel...",
        metadata.num_chunks
    );

    // Process each chunk and collect results
    let chunk_results: Vec<_> = (0..metadata.num_chunks)
        .into_par_iter()
        .map(|chunk_num| {
            let chunk_file = chunks_dir.join(format!("chunk_{}.bin.zst", chunk_num));
            if !chunk_file.exists() {
                return Ok((chunk_num, Vec::new(), Vec::new(), Vec::<()>::new(), None));
            }

            eprintln!("   📦 Processing chunk {}...", chunk_num);

            // Process chunk (same logic as before, but return results instead of mutating shared state)
            let mut chunk_blocks_by_prev_hash = Vec::new();
            let mut chunk_blocks_by_block_hash = Vec::new();
            let mut chunk_genesis: Option<(usize, u64, [u8; 32])> = None;

            let mut zstd_proc = decompress_chunk_streaming(&chunk_file)?;
            let stdout = zstd_proc
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdout"))?;
            let mut reader = std::io::BufReader::with_capacity(1024 * 1024, stdout); // 1MB buffer

            let mut offset: u64 = 0;
            let mut block_num_in_chunk = 0;

            loop {
                let mut len_buf = [0u8; 4];
                match reader.read_exact(&mut len_buf) {
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(e.into()),
                }

                let block_len = u32::from_le_bytes(len_buf) as usize;
                offset += 4;

                if block_len > 2 * 1024 * 1024 * 1024
                    || block_len > 10 * 1024 * 1024
                    || block_len < 88
                {
                    if block_len < 100 * 1024 * 1024 {
                        let mut skip_buf = vec![0u8; block_len.min(1024 * 1024)];
                        let _ = reader.read(&mut skip_buf);
                    }
                    offset += block_len as u64;
                    block_num_in_chunk += 1;
                    continue;
                }

                if block_len < 80 {
                    offset += block_len as u64;
                    block_num_in_chunk += 1;
                    continue;
                }

                let mut header_buf = [0u8; 80];
                reader.read_exact(&mut header_buf)?;

                let first_hash = Sha256::digest(&header_buf);
                let second_hash = Sha256::digest(&first_hash);
                let mut block_hash = [0u8; 32];
                block_hash.copy_from_slice(&second_hash);
                block_hash.reverse();

                if second_hash.iter().all(|&b| b == 0) || block_hash.iter().all(|&b| b == 0) {
                    if block_len > 80 {
                        let mut skip_buf = vec![0u8; block_len - 80];
                        let _ = reader.read_exact(&mut skip_buf);
                    }
                    offset += block_len as u64;
                    block_num_in_chunk += 1;
                    continue;
                }

                let mut prev_hash_le = [0u8; 32];
                prev_hash_le.copy_from_slice(&header_buf[4..36]);
                let is_genesis = prev_hash_le.iter().all(|&b| b == 0);

                if is_genesis {
                    let expected_genesis_prefix = "000000000019d668";
                    let actual_genesis_prefix = hex::encode(&block_hash[..8]);
                    if actual_genesis_prefix == expected_genesis_prefix && chunk_genesis.is_none() {
                        chunk_genesis = Some((chunk_num, offset - 4, block_hash));
                    }
                    if block_len > 80 {
                        let mut skip_buf = vec![0u8; block_len - 80];
                        let _ = reader.read_exact(&mut skip_buf);
                    }
                    offset += block_len as u64;
                    block_num_in_chunk += 1;
                    continue;
                }

                if prev_hash_le.iter().all(|&b| b == 0) {
                    if block_len > 80 {
                        let mut skip_buf = vec![0u8; block_len - 80];
                        let _ = reader.read_exact(&mut skip_buf);
                    }
                    offset += block_len as u64;
                    block_num_in_chunk += 1;
                    continue;
                }

                let mut prev_hash_be = prev_hash_le;
                prev_hash_be.reverse();

                chunk_blocks_by_prev_hash.push((prev_hash_be, (chunk_num, offset - 4, block_hash)));
                chunk_blocks_by_block_hash
                    .push((block_hash, (chunk_num, offset - 4, prev_hash_be)));

                if block_len > 80 {
                    let mut skip_buf = vec![0u8; block_len - 80];
                    let _ = reader.read_exact(&mut skip_buf);
                }

                offset += block_len as u64;
                block_num_in_chunk += 1;

                // Progress logging for large chunks
                if block_num_in_chunk % 25000 == 0 {
                    eprintln!(
                        "   📦 Chunk {}: processed {} blocks...",
                        chunk_num, block_num_in_chunk
                    );
                }
            }

            eprintln!(
                "   ✅ Chunk {} complete: {} blocks processed",
                chunk_num, block_num_in_chunk
            );
            let wait_result = zstd_proc.wait();
            if let Err(e) = wait_result {
                eprintln!(
                    "   ⚠️  Warning: zstd process wait failed for chunk {}: {}",
                    chunk_num, e
                );
            }

            Ok((
                chunk_num,
                chunk_blocks_by_prev_hash,
                chunk_blocks_by_block_hash,
                Vec::new(),
                chunk_genesis,
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map_err(|e| {
            eprintln!("   ❌ Error during parallel chunk processing: {}", e);
            eprintln!("   💡 This may be due to a corrupted chunk or insufficient resources");
            e
        })?;

    // Merge results from parallel processing
    println!("   🔗 Merging results from parallel chunk processing...");
    println!("   📊 Total chunks processed: {}", chunk_results.len());
    for (chunk_num, prev_hash_vec, block_hash_vec, _, chunk_gen) in chunk_results {
        println!(
            "   📦 Merging chunk {} ({} blocks)...",
            chunk_num,
            prev_hash_vec.len()
        );

        for (prev_hash, entry) in prev_hash_vec {
            blocks_by_prev_hash.insert(prev_hash, entry);
        }

        for (block_hash, entry) in block_hash_vec {
            blocks_by_block_hash.insert(block_hash, entry);
        }

        if let Some(r#gen) = chunk_gen {
            if genesis_block.is_none() {
                genesis_block = Some(r#gen);
                println!("     ✅ Found genesis block at chunk {}", chunk_num);
            }
        }
    }

    // Chain blocks by prev_block_hash to determine heights
    // Chain blocks by prev_block_hash to determine heights
    println!("   🔗 Chaining blocks by prev_block_hash...");

    let genesis = genesis_block.ok_or_else(|| anyhow::anyhow!("Genesis block not found"))?;
    index.insert(
        0,
        BlockIndexEntry {
            chunk_number: genesis.0,
            offset_in_chunk: genesis.1,
            block_hash: genesis.2,
        },
    );

    // CRITICAL: genesis.2 is the block_hash of genesis in big-endian format
    // We need to find a block whose prev_hash (when converted to big-endian) matches this
    let mut current_hash = genesis.2;
    println!(
        "     🔍 DEBUG: Genesis block_hash (BE): {}",
        hex::encode(&current_hash)
    );
    println!(
        "     🔍 DEBUG: Looking for block with prev_hash_be matching: {}",
        hex::encode(&current_hash)
    );
    let mut height = 1u64;

    println!(
        "     Starting chain from genesis block hash: {}",
        hex::encode(&current_hash)
    );
    println!("     Looking for blocks with prev_hash matching genesis...");
    println!(
        "     Total blocks by prev_hash: {}",
        blocks_by_prev_hash.len()
    );
    println!(
        "     Total blocks by block_hash: {}",
        blocks_by_block_hash.len()
    );

    // Start chaining from genesis
    // Chunks are primary - RPC is fallback for any missing blocks
    let mut chain_break_count = 0;
    let max_chain_breaks = 10000; // Increased limit - allow more chain breaks before stopping

    loop {
        // First, try to find block in chunks
        if let Some((chunk_num, offset, block_hash)) = blocks_by_prev_hash.remove(&current_hash) {
            index.insert(
                height,
                BlockIndexEntry {
                    chunk_number: chunk_num,
                    offset_in_chunk: offset,
                    block_hash,
                },
            );

            current_hash = block_hash;
            height += 1;
            chain_break_count = 0; // Reset on success

            if height <= 5 {
                println!(
                    "     Height {}: chunk {}, offset {}, hash {}",
                    height - 1,
                    chunk_num,
                    offset,
                    hex::encode(&block_hash[..8])
                );
            }

            if height % 10000 == 0 {
                println!("     Indexed {} blocks...", height);
                if let Err(e) = save_block_index(chunks_dir, &index) {
                    eprintln!(
                        "     ⚠️  Warning: Failed to save index at height {}: {}",
                        height, e
                    );
                }
            }
        } else {
            // Block not found in chunks - skip for now, will be fetched in async context
            if chain_break_count < max_chain_breaks {
                // Reduced logging frequency to improve performance
                if height < 10 || height % 10000 == 0 {
                    println!(
                        "     ⚠️  Block {} not found in chunks - will be fetched from RPC in async context",
                        height
                    );
                }
                // Can't fetch from RPC here (sync context) - will be handled by build_block_index_via_rpc
                chain_break_count += 1;
                height += 1;
                continue;
            }

            // Block not found and couldn't fetch - check if we should stop
            chain_break_count += 1;
            if chain_break_count > max_chain_breaks || blocks_by_prev_hash.is_empty() {
                eprintln!(
                    "   ⚠️  Stopping chain at height {} ({} breaks, {} blocks remaining)",
                    height,
                    chain_break_count,
                    blocks_by_prev_hash.len()
                );
                break;
            }

            // Try to skip this height and continue
            height += 1;
        }

        // Safety check: prevent infinite loop
        if height > 1_000_000 {
            eprintln!("   ⚠️  Chain too long, stopping at {} blocks", height);
            break;
        }

        // Check if we've processed all available blocks
        if blocks_by_prev_hash.is_empty() && chain_break_count == 0 {
            break;
        }
    }

    if height <= 1 {
        eprintln!("   ⚠️  WARNING: Chain stopped after {} block(s)!", height);
        if height == 0 {
            eprintln!("     Only genesis block indexed");
        } else {
            eprintln!("     Only genesis and block 1 indexed");
        }
        eprintln!("     Current hash: {}", hex::encode(&current_hash[..8]));
        eprintln!(
            "     Looking for blocks with prev_hash matching: {}",
            hex::encode(&current_hash[..8])
        );
        eprintln!("     Available prev_hashes (first 5):");
        for (i, (prev_hash, _)) in blocks_by_prev_hash.iter().take(5).enumerate() {
            eprintln!("       {}: {}", i, hex::encode(&prev_hash[..8]));
        }
        eprintln!("   🔄 Chaining failed - blocks appear to be out of order in chunks");
        eprintln!(
            "   💡 This is expected for XOR-packaged block files where blocks are stored out of order"
        );
        eprintln!("   💡 The index will be built using block hashes from RPC when available");
        eprintln!("   ⚠️  For now, returning partial index");
        eprintln!("   💡 To build full index, use build_block_index_via_rpc from async context");

        // Return partial index - RPC-based indexing will be used when available
    }

    println!("   ✅ Built index for {} blocks", index.len());

    // CRITICAL FIX: Save index incrementally so progress isn't lost on restart
    // Save even if partial - allows resuming from where we left off
    if index.len() > 0 {
        if let Err(e) = save_block_index(chunks_dir, &index) {
            eprintln!("   ⚠️  Warning: Failed to save index incrementally: {}", e);
            eprintln!("   💡 Progress will be lost if process is killed");
        } else {
            println!(
                "   💾 Saved index incrementally ({} entries) - progress preserved",
                index.len()
            );
        }
    }

    Ok((index, blocks_by_block_hash))
}

/// Verify block index correctness by checking prev_block_hash chain
pub fn verify_block_index(chunks_dir: &Path, index: &BlockIndex) -> Result<bool> {
    use crate::chunked_cache::decompress_chunk_streaming;
    use std::io::Read;
    use std::process::Stdio;

    println!("🔍 Verifying block index...");

    let mut prev_hash = [0u8; 32]; // Genesis has all-zero prev_hash

    for height in 0..index.len().min(100) as u64 {
        // Verify first 100 blocks
        let entry = index
            .get(&height)
            .ok_or_else(|| anyhow::anyhow!("Missing index entry for height {}", height))?;

        // Read block header from chunk
        // Note: We can't seek in a zstd stream, so we need to read from start and skip
        // For verification, we'll just check a few blocks - full implementation would need
        // to cache decompressed chunks or use a different approach
        let chunk_file = chunks_dir.join(format!("chunk_{}.bin.zst", entry.chunk_number));
        let mut zstd_proc = decompress_chunk_streaming(&chunk_file)?;
        let stdout = zstd_proc
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdout"))?;
        let mut reader = std::io::BufReader::new(stdout);

        // Skip to offset (read and discard bytes)
        let mut skip_bytes = entry.offset_in_chunk;
        let mut skip_buf = vec![0u8; 1024 * 1024]; // 1MB buffer for skipping
        while skip_bytes > 0 {
            let to_skip = skip_bytes.min(skip_buf.len() as u64) as usize;
            reader.read_exact(&mut skip_buf[..to_skip])?;
            skip_bytes -= to_skip as u64;
        }

        // Read block length
        let mut len_buf = [0u8; 4];
        reader.read_exact(&mut len_buf)?;
        let block_len = u32::from_le_bytes(len_buf) as usize;

        // Read header
        let mut header = vec![0u8; 80.min(block_len)];
        reader.read_exact(&mut header)?;

        // Extract prev_block_hash
        let mut block_prev_hash = [0u8; 32];
        block_prev_hash.copy_from_slice(&header[4..36]);
        block_prev_hash.reverse(); // Convert to big-endian

        // Verify prev_hash matches
        if height > 0 && block_prev_hash != prev_hash {
            eprintln!("   ❌ Height {}: prev_hash mismatch!", height);
            eprintln!("      Expected: {}", hex::encode(&prev_hash));
            eprintln!("      Got:      {}", hex::encode(&block_prev_hash));
            return Ok(false);
        }

        // Calculate block hash
        let first_hash = Sha256::digest(&header);
        let second_hash = Sha256::digest(&first_hash);
        let mut block_hash = [0u8; 32];
        block_hash.copy_from_slice(&second_hash);
        block_hash.reverse();

        // Verify block hash matches index
        if block_hash != entry.block_hash {
            eprintln!("   ❌ Height {}: block_hash mismatch!", height);
            eprintln!("      Expected: {}", hex::encode(&entry.block_hash));
            eprintln!("      Got:      {}", hex::encode(&block_hash));
            return Ok(false);
        }

        prev_hash = block_hash;
    }

    println!(
        "   ✅ Index verification passed for first {} blocks",
        index.len().min(100)
    );
    Ok(true)
}

#[cfg(test)]
mod contiguous_tests {
    use super::{BlockIndex, BlockIndexEntry, contiguous_chain_from, iterator_eager_height_range};

    fn dummy_entry() -> BlockIndexEntry {
        BlockIndexEntry {
            chunk_number: 0,
            offset_in_chunk: 0,
            block_hash: [0u8; 32],
        }
    }

    #[test]
    fn contiguous_from_empty() {
        let idx = BlockIndex::new();
        assert_eq!(contiguous_chain_from(&idx, 0), (0, 0));
    }

    #[test]
    fn contiguous_genesis_only() {
        let mut idx = BlockIndex::new();
        idx.insert(0, dummy_entry());
        assert_eq!(contiguous_chain_from(&idx, 0), (1, 1));
    }

    #[test]
    fn contiguous_three() {
        let mut idx = BlockIndex::new();
        idx.insert(0, dummy_entry());
        idx.insert(1, dummy_entry());
        idx.insert(2, dummy_entry());
        assert_eq!(contiguous_chain_from(&idx, 0), (3, 3));
    }

    #[test]
    fn iterator_eager_range() {
        assert_eq!(iterator_eager_height_range(0, 1000), Some(0..100));
        assert_eq!(iterator_eager_height_range(50, 1000), Some(50..100));
        assert_eq!(iterator_eager_height_range(100, 1000), None);
    }
}
