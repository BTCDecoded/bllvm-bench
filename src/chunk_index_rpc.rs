//! Alternative block index builder using node RPC
//!
//! If chaining fails (blocks out of order), use RPC to get block hashes
//! by height, then search chunks for those specific hashes.

use crate::chunk_index::{BlockIndex, BlockIndexEntry};
use crate::chunked_cache::{decompress_chunk_streaming, load_chunk_metadata};
use crate::remote_core_rpc::RemoteCoreRpcClient;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// Build block index using node RPC to get block hashes by height
/// OPTIMIZED: First builds hash map from chunks (fast), then uses it for O(1) lookups
pub async fn build_block_index_via_rpc(
    chunks_dir: &Path,
    max_height: Option<u64>,
) -> Result<BlockIndex> {
    println!("🔨 Building block index via RPC (optimized)...");

    let rpc_client = RemoteCoreRpcClient::new();

    // Get chain height
    let chain_height = rpc_client
        .get_block_count()
        .await
        .context("Failed to get chain height from node")?;
    let max_h = max_height.unwrap_or(chain_height).min(chain_height);

    println!("   Chain height: {}", chain_height);
    println!("   Indexing up to height: {}", max_h);

    // OPTIMIZATION: Try to load existing index and hash map first (resume from previous run)
    use crate::chunk_index::{
        BlockHashMap, build_block_index, load_block_index, load_hash_map, save_hash_map,
    };
    use std::collections::HashMap;

    // CRITICAL FIX: Load existing index FIRST, BEFORE building hash map
    // This ensures we preserve all existing work before any operations that might touch the index file
    let mut index = match load_block_index(chunks_dir)? {
        Some(existing_index) if existing_index.len() > 1 => {
            println!(
                "   ✅ Loaded existing index ({} entries) - will preserve and extend",
                existing_index.len()
            );
            existing_index
        }
        _ => {
            println!("   📝 No existing index - starting fresh");
            HashMap::new()
        }
    };
    let existing_count = index.len();

    // Now load or build hash map (AFTER loading index to preserve it)
    let blocks_by_hash: BlockHashMap = match load_hash_map(chunks_dir)? {
        Some(saved_hash_map) => {
            println!(
                "   ✅ Loaded hash map from disk ({} entries) - skipping Step 1!",
                saved_hash_map.len()
            );
            saved_hash_map
        }
        None => {
            // Hash map not saved yet - need to build it from chunks (Step 1)
            println!("   🚀 Step 1: Building hash map from chunks for fast lookups...");
            println!(
                "   💡 Note: Existing index ({} entries) will be preserved",
                existing_count
            );
            let (_, hash_map) = build_block_index(chunks_dir)
                .with_context(|| "Failed to build hash map from chunks")?;

            // Convert and save for next time
            let mut blocks_by_hash: BlockHashMap = HashMap::new();
            for (block_hash, (chunk_num, offset, _)) in hash_map {
                blocks_by_hash.insert(block_hash, (chunk_num, offset));
            }

            // Save hash map for future runs
            if let Err(e) = save_hash_map(chunks_dir, &blocks_by_hash) {
                eprintln!(
                    "   ⚠️  Warning: Failed to save hash map: {} (will rebuild next time)",
                    e
                );
            } else {
                println!(
                    "   💾 Saved hash map ({} entries) - Step 1 will be skipped on next run!",
                    blocks_by_hash.len()
                );
            }

            // CRITICAL: Verify index wasn't corrupted during hash map build
            if index.len() != existing_count {
                eprintln!(
                    "   ❌ ERROR: Index was modified during hash map build! ({} -> {} entries)",
                    existing_count,
                    index.len()
                );
                eprintln!("   💡 This is a bug - please report it");
            } else {
                println!("   ✅ Index preserved ({} entries)", index.len());
            }

            blocks_by_hash
        }
    };

    println!(
        "   ✅ Using hash map with {} blocks for fast lookups",
        blocks_by_hash.len()
    );

    // Step 2: For each height, get block hash from RPC, then look up in hash map (O(1))
    println!("   🚀 Step 2: Indexing remaining blocks by height using hash map (fast lookups)...");

    // OPTIMIZATION: Process blocks in batches with parallel RPC calls
    // AGGRESSIVE: On LAN, we can handle much higher concurrency
    // CPU is ~67% idle, so we can push harder
    const BATCH_SIZE: usize = 150; // Higher batch for 100% RPC scenario
    const MISSING_BLOCK_CONCURRENCY: usize = 75; // Higher concurrency - SSH is fresh

    println!(
        "   💡 Using parallel RPC calls (batch size: {}) for faster processing",
        BATCH_SIZE
    );
    println!(
        "   💡 Missing block fetch concurrency: {}",
        MISSING_BLOCK_CONCURRENCY
    );

    // Test RPC connection before starting
    println!("   🔍 Testing RPC connection...");
    match rpc_client.get_block_hash(0).await {
        Ok(_) => println!("   ✅ RPC connection working"),
        Err(e) => {
            eprintln!("   ❌ RPC connection failed: {}", e);
            eprintln!("   ⚠️  Continuing anyway, but RPC calls may fail");
        }
    }
    let mut last_save_height = 0u64;
    const SAVE_INTERVAL: u64 = 50000; // Save index every 50k blocks (reduce I/O, still safe with frequent saves)

    // OPTIMIZATION: Pre-compute missing heights set for O(1) lookup instead of iterating
    // This avoids checking index.contains_key() for every height in the loop
    use std::collections::BTreeSet;
    println!(
        "   🔍 Pre-computing missing heights (checking {} heights)...",
        max_h + 1
    );
    let mut missing_heights: BTreeSet<u64> = BTreeSet::new();
    let start_time = std::time::Instant::now();
    for h in 0..=max_h {
        if !index.contains_key(&h) {
            missing_heights.insert(h);
        }
        // Progress update every 25k heights
        if h % 25000 == 0 && h > 0 {
            let elapsed = start_time.elapsed();
            let rate = h as f64 / elapsed.as_secs_f64();
            let remaining = (max_h - h) as f64 / rate;
            println!(
                "   📊 Pre-computing progress: {}/{} ({:.1}%), {} missing so far, ~{:.0}s remaining",
                h,
                max_h,
                (h as f64 / (max_h + 1) as f64) * 100.0,
                missing_heights.len(),
                remaining
            );
        }
    }
    let elapsed = start_time.elapsed();
    println!(
        "   ✅ Pre-computation complete in {:.1}s ({} missing heights found)",
        elapsed.as_secs_f64(),
        missing_heights.len()
    );

    let missing_count = missing_heights.len() as u64;
    if missing_count > 0 {
        println!(
            "   📊 Need to index {} blocks ({} already in index)",
            missing_count,
            index.len()
        );
    } else {
        println!("   ✅ All blocks already indexed!");
        return Ok(index);
    }

    // OPTIMIZATION: Process missing heights directly instead of iterating all heights
    let mut missing_iter = missing_heights.iter().copied();
    let mut batch_heights_vec = Vec::with_capacity(BATCH_SIZE);

    let initial_missing_count = missing_heights.len();
    let mut processed_count = 0usize;

    loop {
        // Collect batch of missing heights directly (no need to check index.contains_key)
        batch_heights_vec.clear();
        while batch_heights_vec.len() < BATCH_SIZE {
            match missing_iter.next() {
                Some(h) => batch_heights_vec.push(h),
                None => break, // No more missing heights
            }
        }

        if batch_heights_vec.is_empty() {
            // Check if we actually indexed all required blocks
            let expected_count = (max_h + 1) as usize;
            let actual_count = index.len();
            if actual_count < expected_count {
                let missing = expected_count - actual_count;
                println!(
                    "   🔄 Iterator exhausted but {} blocks still missing - recomputing missing heights...",
                    missing
                );

                // Recompute missing heights to retry failed blocks
                missing_heights.clear();
                for h in 0..=max_h {
                    if !index.contains_key(&h) {
                        missing_heights.insert(h);
                    }
                }

                if missing_heights.is_empty() {
                    // Actually complete now
                    println!(
                        "   ✅ All missing blocks processed! ({}/{} blocks indexed)",
                        actual_count, expected_count
                    );
                    break;
                } else {
                    // Restart iterator with newly computed missing heights
                    missing_iter = missing_heights.iter().copied();
                    println!(
                        "   🔄 Found {} missing blocks - retrying...",
                        missing_heights.len()
                    );
                    continue; // Continue loop with new iterator
                }
            } else {
                println!(
                    "   ✅ All missing blocks processed! ({}/{} blocks indexed)",
                    actual_count, expected_count
                );
                break;
            }
        }

        let batch_heights = &batch_heights_vec;
        let current_height = batch_heights[0];
        processed_count += batch_heights_vec.len();
        let remaining = initial_missing_count.saturating_sub(processed_count);

        // Progress update (less frequent to reduce overhead)
        if current_height % 20000 == 0 || batch_heights_vec.len() < BATCH_SIZE {
            println!(
                "   📊 Progress: {} blocks indexed, ~{} remaining (height ~{}, batch size: {})",
                index.len(),
                remaining,
                current_height,
                batch_heights_vec.len()
            );
        }

        // DEBUG: Log batch collection
        if processed_count <= 20000 || processed_count % 50000 == 0 {
            println!(
                "   🔍 DEBUG: Collected batch of {} heights (starting at {}), {} total processed",
                batch_heights_vec.len(),
                current_height,
                processed_count
            );
        }

        // OPTIMIZATION: Use batch RPC call - single SSH round-trip for all heights!
        use tokio::time::{Duration as TokioDuration, timeout};
        const BATCH_TIMEOUT: TokioDuration = TokioDuration::from_secs(60); // 1 minute for batch (single call)

        if processed_count <= 20000 || processed_count % 50000 == 0 {
            println!(
                "   🔍 DEBUG: Making BATCH RPC call for {} heights (single SSH round-trip)",
                batch_heights_vec.len()
            );
        }

        // Single batch RPC call for all heights in this batch
        let batch_results: Vec<std::result::Result<(u64, String), (u64, anyhow::Error)>> =
            match timeout(
                BATCH_TIMEOUT,
                rpc_client.get_block_hashes_batch(&batch_heights_vec),
            )
            .await
            {
                Ok(Ok(results)) => {
                    // Convert batch results to expected format
                    results
                        .into_iter()
                        .map(|(h, r)| match r {
                            Ok(hash) => Ok((h, hash)),
                            Err(e) => Err((h, e)),
                        })
                        .collect()
                }
                Ok(Err(e)) => {
                    eprintln!("   ⚠️  Batch RPC failed: {} - will retry individually", e);
                    vec![] // Empty results, will be retried
                }
                Err(_) => {
                    eprintln!(
                        "   ⚠️  Batch RPC timeout after {:.0}s",
                        BATCH_TIMEOUT.as_secs()
                    );
                    vec![]
                }
            };

        if processed_count <= 20000 || processed_count % 50000 == 0 {
            println!(
                "   🔍 DEBUG: RPC batch completed, processing {} results...",
                batch_results.len()
            );
        }

        // OPTIMIZATION: Collect missing blocks and fetch them in parallel
        let mut missing_blocks_to_fetch: Vec<(u64, [u8; 32])> = Vec::new();

        if batch_heights[0] % 10000 == 0 {
            println!("   🔍 Processing batch results to identify missing blocks...");
        }

        // Process results - first pass: identify missing blocks
        if batch_results.is_empty() {
            eprintln!(
                "   ⚠️  WARNING: No results from batch - batch may have timed out or all calls failed"
            );
            eprintln!("   💡 Skipping this batch and continuing to next batch");
            // Continue to next batch iteration
            continue;
        }

        for result in batch_results {
            let (block_height, block_hash_hex) = match result {
                Ok((h, hash)) => (h, hash),
                Err((h, _)) => {
                    // RPC failed - skip this block
                    continue;
                }
            };

            // Bitcoin RPC returns hash as hex string in DISPLAY format (big-endian)
            // Example: "000000000019d668..." - when decoded, gives big-endian bytes
            let block_hash_bytes = match hex::decode(&block_hash_hex) {
                Ok(bytes) => bytes,
                Err(e) => {
                    if block_height < 100 || block_height % 1000 == 0 {
                        eprintln!(
                            "   ⚠️  Failed to decode block hash hex for height {}: {} - skipping",
                            block_height, e
                        );
                    }
                    continue;
                }
            };
            if block_hash_bytes.len() != 32 {
                if block_height < 100 || block_height % 1000 == 0 {
                    eprintln!(
                        "   ⚠️  Invalid block hash length for height {}: {} bytes - skipping",
                        block_height,
                        block_hash_bytes.len()
                    );
                }
                continue;
            }

            // CRITICAL FIX: Bitcoin RPC returns hash in DISPLAY format (big-endian hex string)
            // Try big-endian first (standard RPC format)
            let mut block_hash_be = [0u8; 32];
            block_hash_be.copy_from_slice(&block_hash_bytes);

            // OPTIMIZATION: Look up in hash map (O(1)) - RPC returns big-endian format
            // We confirmed this works, so no need to check little-endian (saves one hash lookup)
            let found = blocks_by_hash
                .get(&block_hash_be)
                .map(|(chunk_num, offset)| (*chunk_num, *offset));

            if let Some((chunk_num, offset)) = found {
                // Found in chunks - add to index
                index.insert(
                    block_height,
                    BlockIndexEntry {
                        chunk_number: chunk_num,
                        offset_in_chunk: offset,
                        block_hash: block_hash_be, // Always store as big-endian
                    },
                );
            } else {
                // Not found in chunks - collect for parallel fetching
                missing_blocks_to_fetch.push((block_height, block_hash_be));
            }
        }

        // OPTIMIZATION: Fetch missing blocks in parallel (with concurrency limit)
        if !missing_blocks_to_fetch.is_empty() {
            const FETCH_TIMEOUT: TokioDuration = TokioDuration::from_secs(10); // 10 second timeout (LAN is fast)

            println!(
                "   🔍 DEBUG: Starting to fetch {} missing blocks for batch starting at height {}...",
                missing_blocks_to_fetch.len(),
                current_height
            );

            // Fetch missing blocks in parallel batches (to avoid overwhelming RPC)
            let num_chunks = (missing_blocks_to_fetch.len() + MISSING_BLOCK_CONCURRENCY - 1)
                / MISSING_BLOCK_CONCURRENCY;

            for (chunk_idx, chunk_start) in (0..missing_blocks_to_fetch.len())
                .step_by(MISSING_BLOCK_CONCURRENCY)
                .enumerate()
            {
                let chunk_end =
                    (chunk_start + MISSING_BLOCK_CONCURRENCY).min(missing_blocks_to_fetch.len());
                let chunk = &missing_blocks_to_fetch[chunk_start..chunk_end];

                // Create futures for this chunk with timeout protection
                let mut chunk_futures = Vec::new();
                for (block_height, block_hash_be) in chunk.iter() {
                    let height = *block_height;
                    let hash = *block_hash_be;
                    let chunks_dir = chunks_dir.to_path_buf();
                    let rpc_client_ref = &rpc_client;

                    chunk_futures.push(async move {
                        // Reduced logging frequency
                        if height < 100 || height % 10000 == 0 {
                            eprintln!("   ⚠️  Block {} (hash: {}) not found in hash map - fetching from RPC", 
                                     height, hex::encode(&hash[..8]));
                        }

                        // Wrap fetch in timeout to prevent hanging
                        match timeout(FETCH_TIMEOUT, crate::missing_blocks::fetch_and_store_missing_block(&chunks_dir, height, rpc_client_ref)).await {
                            Ok(Ok(offset)) => Ok((height, offset, hash)),
                            Ok(Err(e)) => {
                                if height < 100 || height % 10000 == 0 {
                                    eprintln!("   ⚠️  Failed to fetch missing block {}: {}", height, e);
                                }
                                Err((height, e))
                            }
                            Err(_) => {
                                // Timeout
                                if height < 100 || height % 10000 == 0 {
                                    eprintln!("   ⚠️  Timeout fetching missing block {} (>{:?})", height, FETCH_TIMEOUT);
                                }
                                Err((height, anyhow::anyhow!("Timeout after {:?}", FETCH_TIMEOUT)))
                            }
                        }
                    });
                }

                // Execute chunk in parallel with per-chunk timeout
                const CHUNK_TIMEOUT: TokioDuration = TokioDuration::from_secs(60); // 1 minute timeout per chunk (LAN is fast)

                if chunk_idx % 5 == 0 || chunk_idx == 0 {
                    println!(
                        "   🔍 DEBUG: Executing missing block chunk {}/{} ({} blocks) with {:.0}s timeout...",
                        chunk_idx + 1,
                        num_chunks,
                        chunk.len(),
                        CHUNK_TIMEOUT.as_secs()
                    );
                }

                let chunk_results = match timeout(
                    CHUNK_TIMEOUT,
                    futures::future::join_all(chunk_futures),
                )
                .await
                {
                    Ok(results) => results,
                    Err(_) => {
                        eprintln!(
                            "   ⚠️  WARNING: Chunk {}/{} timed out after {:.0}s - some blocks may not have been fetched",
                            chunk_idx + 1,
                            num_chunks,
                            CHUNK_TIMEOUT.as_secs()
                        );
                        eprintln!(
                            "   💡 Continuing with next chunk - failed blocks will be retried on next run"
                        );
                        // Return empty results to skip this chunk
                        vec![]
                    }
                };

                // Process results
                for result in chunk_results {
                    match result {
                        Ok((block_height, offset, block_hash_be)) => {
                            index.insert(
                                block_height,
                                BlockIndexEntry {
                                    chunk_number: 999, // Missing blocks chunk
                                    offset_in_chunk: offset,
                                    block_hash: block_hash_be,
                                },
                            );
                            // Reduced logging frequency
                            if block_height < 10 || block_height % 10000 == 0 {
                                println!(
                                    "   ✅ Stored missing block {} in chunk_missing (offset: {})",
                                    block_height, offset
                                );
                            }
                        }
                        Err((height, e)) => {
                            // Error already logged above
                            if height < 100 || height % 10000 == 0 {
                                eprintln!("   ⚠️  Skipping block {} due to error: {}", height, e);
                            }
                        }
                    }
                }

                // CRITICAL: Save index after each batch of missing blocks to prevent data loss
                use crate::chunk_index::save_block_index;
                if let Err(e) = save_block_index(chunks_dir, &index) {
                    eprintln!(
                        "   ⚠️  Warning: Failed to save index after missing blocks batch: {}",
                        e
                    );
                } else {
                    println!(
                        "   💾 Saved index after missing blocks batch ({} entries) - progress preserved",
                        index.len()
                    );
                    last_save_height = index.len() as u64;
                }

                // DEBUG: Log progress within missing block fetching
                if chunk_idx % 10 == 0 || chunk_idx == num_chunks - 1 {
                    println!(
                        "   🔍 DEBUG: Processed missing block chunk {}/{} ({} total missing blocks in this batch)",
                        chunk_idx + 1,
                        num_chunks,
                        missing_blocks_to_fetch.len()
                    );
                }
            }

            // DEBUG: Log completion of missing block fetching for this batch
            println!(
                "   🔍 DEBUG: Completed fetching {} missing blocks for batch starting at height {}",
                missing_blocks_to_fetch.len(),
                current_height
            );
        } else {
            // DEBUG: Log when no missing blocks found in batch
            if processed_count <= 20000 || processed_count % 50000 == 0 {
                println!(
                    "   🔍 DEBUG: No missing blocks in batch starting at height {} (all found in chunks)",
                    current_height
                );
            }
        }

        // DEBUG: Log loop continuation - CRITICAL to see if loop continues
        println!(
            "   🔍 DEBUG: Completed batch processing for heights starting at {}, continuing to next batch... (processed: {}, remaining: {})",
            current_height, processed_count, remaining
        );

        // Missing block fetching is now handled above with timeout protection

        // OPTIMIZATION: Save index periodically to preserve progress
        // Save every SAVE_INTERVAL blocks to preserve progress
        if index.len() as u64 - last_save_height >= SAVE_INTERVAL {
            println!(
                "   💾 Saving index ({} entries, +{} since last save)...",
                index.len(),
                index.len() as u64 - last_save_height
            );
            use crate::chunk_index::save_block_index;
            if let Err(e) = save_block_index(chunks_dir, &index) {
                eprintln!("   ❌ ERROR: Failed to save index: {}", e);
            } else {
                println!(
                    "   ✅ Saved index ({} entries) - progress preserved",
                    index.len()
                );
                last_save_height = index.len() as u64;
            }
        }
    }

    let expected_count = (max_h + 1) as usize;
    let actual_count = index.len();
    println!(
        "   🏁 Main loop completed: {} blocks indexed (expected: {})",
        actual_count, expected_count
    );

    if actual_count < expected_count {
        let missing = expected_count - actual_count;
        eprintln!(
            "   ⚠️  WARNING: Index incomplete! Only {}/{} blocks indexed ({} missing)",
            actual_count, expected_count, missing
        );
        eprintln!("   💡 Some RPC calls failed and those blocks were skipped");
        eprintln!("   💡 Restart the process to retry failed blocks");
    } else {
        println!("   ✅ Index complete! All {} blocks indexed", actual_count);
    }

    println!("   ✅ Built index for {} blocks", index.len());
    Ok(index)
}

/// Find a specific block in a chunk by its hash
async fn find_block_in_chunk(
    chunk_file: &Path,
    block_hash_be: &[u8; 32],
    _block_hash_le: &[u8; 32],
    chunk_num: usize,
) -> Result<BlockIndexEntry> {
    let mut zstd_proc = decompress_chunk_streaming(chunk_file)?;
    let stdout = zstd_proc
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdout"))?;
    let mut reader = std::io::BufReader::new(stdout);

    let mut offset: u64 = 0;

    loop {
        let mut len_buf = [0u8; 4];
        match reader.read_exact(&mut len_buf) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break; // End of chunk
            }
            Err(e) => return Err(e.into()),
        }

        let block_len = u32::from_le_bytes(len_buf) as usize;
        offset += 4;

        if block_len > 10 * 1024 * 1024 || block_len < 88 {
            break; // Invalid
        }

        // Read header
        let mut header_buf = vec![0u8; 80.min(block_len)];
        reader.read_exact(&mut header_buf)?;

        // Calculate block hash
        let first_hash = Sha256::digest(&header_buf);
        let second_hash = Sha256::digest(&first_hash);
        let mut calculated_hash = [0u8; 32];
        calculated_hash.copy_from_slice(&second_hash);
        calculated_hash.reverse(); // Big-endian

        // Check if this is the block we're looking for
        // Compare both BE and LE formats to be sure
        if calculated_hash == *block_hash_be {
            return Ok(BlockIndexEntry {
                chunk_number: chunk_num,
                offset_in_chunk: offset - 4,
                block_hash: *block_hash_be,
            });
        }

        // Skip rest of block
        if block_len > 80 {
            let mut skip_buf = vec![0u8; block_len - 80];
            reader.read_exact(&mut skip_buf)?;
        }

        offset += block_len as u64;
    }

    // Wait for zstd to finish
    let _ = zstd_proc.wait();

    anyhow::bail!("Block not found in chunk {}", chunk_num)
}
