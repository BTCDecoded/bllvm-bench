//! Build hash map from chunks WITHOUT chaining - OPTIMIZED
//! Uses parallel chunk processing and multi-threaded decompression

use anyhow::Result;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn main() -> Result<()> {
    // Force line buffering
    let _ = std::io::stdout().flush();

    let chunks_dir = blvm_bench::require_block_cache_dir()?;

    println!("🔨 Building hash map from ALL chunks (PARALLEL + MT decompression)...");
    println!("   Chunks directory: {}", chunks_dir.display());
    println!("   Available CPUs: {}", num_cpus::get());
    let _ = std::io::stdout().flush();

    // Load metadata
    use blvm_bench::chunked_cache::load_chunk_metadata;
    let metadata = load_chunk_metadata(&chunks_dir)?
        .ok_or_else(|| anyhow::anyhow!("No chunk metadata found"))?;

    println!(
        "   Metadata: {} chunks, {} total blocks",
        metadata.num_chunks, metadata.total_blocks
    );
    let _ = std::io::stdout().flush();

    // Shared results
    let total_blocks: Arc<Mutex<HashMap<[u8; 32], (usize, u64)>>> = Arc::new(Mutex::new(
        HashMap::with_capacity(metadata.total_blocks as usize),
    ));
    let chunk_counts: Arc<Mutex<HashMap<usize, usize>>> = Arc::new(Mutex::new(HashMap::new()));

    // Process chunks in parallel (2-3 at a time to balance I/O and CPU)
    // HDD can only do ~150MB/s, so 2-3 parallel decompressions is optimal
    let chunk_nums: Vec<usize> = (0..metadata.num_chunks).collect();

    // Use rayon with limited parallelism for HDD
    rayon::ThreadPoolBuilder::new()
        .num_threads(3) // 3 parallel chunks (HDD bottleneck)
        .build_global()
        .unwrap_or(());

    println!("   🚀 Starting parallel processing (3 chunks at a time)...");
    let _ = std::io::stdout().flush();

    let start_time = std::time::Instant::now();

    chunk_nums.par_iter().for_each(|&chunk_num| {
        let chunk_file = chunks_dir.join(format!("chunk_{}.bin.zst", chunk_num));
        if !chunk_file.exists() {
            eprintln!(
                "   ⚠️  Chunk {} not found: {}",
                chunk_num,
                chunk_file.display()
            );
            return;
        }

        println!("   📦 Processing chunk {} (MT decompression)...", chunk_num);
        let _ = std::io::stdout().flush();

        // Use multi-threaded decompression (4 threads per chunk)
        use blvm_bench::chunked_cache::decompress_chunk_streaming_mt;
        let mut zstd_proc = match decompress_chunk_streaming_mt(&chunk_file, 4) {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "   ❌ Failed to start decompression for chunk {}: {}",
                    chunk_num, e
                );
                return;
            }
        };

        let stdout = match zstd_proc.stdout.take() {
            Some(s) => s,
            None => {
                eprintln!("   ❌ Failed to get stdout for chunk {}", chunk_num);
                return;
            }
        };

        // Larger buffer for better throughput
        let mut reader = std::io::BufReader::with_capacity(4 * 1024 * 1024, stdout);

        let mut offset: u64 = 0;
        let mut block_count = 0usize;
        let mut local_blocks: Vec<([u8; 32], (usize, u64))> = Vec::with_capacity(125_000);

        loop {
            // Read block length
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    eprintln!("   ❌ Read error in chunk {}: {}", chunk_num, e);
                    break;
                }
            }

            let block_len = u32::from_le_bytes(len_buf) as usize;
            let block_start = offset;
            offset += 4;

            if block_len > 10 * 1024 * 1024 || block_len < 80 {
                eprintln!(
                    "   ⚠️  Invalid block length {} at offset {} in chunk {}",
                    block_len, block_start, chunk_num
                );
                break;
            }

            // Read header (80 bytes)
            let mut header_buf = [0u8; 80];
            if let Err(e) = reader.read_exact(&mut header_buf) {
                eprintln!("   ❌ Failed to read header in chunk {}: {}", chunk_num, e);
                break;
            }

            // Calculate block hash (double SHA256 of header, reversed for big-endian)
            let first_hash = Sha256::digest(&header_buf);
            let second_hash = Sha256::digest(&first_hash);
            let mut block_hash = [0u8; 32];
            block_hash.copy_from_slice(&second_hash);
            block_hash.reverse(); // Big-endian

            // Store locally first (avoid lock contention)
            local_blocks.push((block_hash, (chunk_num, block_start)));
            block_count += 1;

            // Skip rest of block
            if block_len > 80 {
                let mut skip_buf = vec![0u8; block_len - 80];
                if let Err(e) = reader.read_exact(&mut skip_buf) {
                    eprintln!(
                        "   ❌ Failed to skip block data in chunk {}: {}",
                        chunk_num, e
                    );
                    break;
                }
            }

            offset += block_len as u64;

            // Progress every 50k blocks
            if block_count % 50000 == 0 {
                println!(
                    "      Chunk {}: {} blocks processed...",
                    chunk_num, block_count
                );
                let _ = std::io::stdout().flush();
            }
        }

        // Merge local results into global
        {
            let mut global = total_blocks.lock().unwrap();
            for (hash, entry) in local_blocks {
                global.insert(hash, entry);
            }
        }
        {
            let mut counts = chunk_counts.lock().unwrap();
            counts.insert(chunk_num, block_count);
        }

        println!("   ✅ Chunk {} complete: {} blocks", chunk_num, block_count);
        let _ = std::io::stdout().flush();
        let _ = zstd_proc.wait();
    });

    let elapsed = start_time.elapsed();

    let total_blocks = Arc::try_unwrap(total_blocks).unwrap().into_inner().unwrap();
    let chunk_counts = Arc::try_unwrap(chunk_counts).unwrap().into_inner().unwrap();

    println!("\n📊 Results (completed in {:.1}s):", elapsed.as_secs_f64());
    println!("   Total unique blocks in hash map: {}", total_blocks.len());
    println!("   Blocks by chunk:");
    for chunk_num in 0..metadata.num_chunks {
        let count = chunk_counts.get(&chunk_num).copied().unwrap_or(0);
        println!("      Chunk {}: {} blocks", chunk_num, count);
    }
    let _ = std::io::stdout().flush();

    // Check for duplicates
    let total_processed: usize = chunk_counts.values().sum();
    let duplicates = total_processed.saturating_sub(total_blocks.len());
    if duplicates > 0 {
        println!(
            "   ⚠️  Duplicate blocks (same hash in multiple chunks): {}",
            duplicates
        );
    }

    // Save hash map
    use blvm_bench::chunk_index::{BlockHashMap, save_hash_map};
    let hash_map: BlockHashMap = total_blocks;

    // Backup existing
    let hash_map_file = chunks_dir.join("chunks.hashmap");
    if hash_map_file.exists() {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let backup = chunks_dir.join(format!("chunks.hashmap.backup.{}", timestamp));
        std::fs::copy(&hash_map_file, &backup)?;
        println!("   💾 Backed up existing hash map to {}", backup.display());
    }

    save_hash_map(&chunks_dir, &hash_map)?;
    println!("   💾 Saved new hash map ({} entries)", hash_map.len());
    println!("\n✅ Hash map build complete!");
    let _ = std::io::stdout().flush();

    Ok(())
}
