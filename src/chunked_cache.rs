//! Chunked and compressed cache support
//!
//! Handles reading from chunked, compressed cache files created by split_and_compress_cache.sh
//! Format: Multiple files like chunk_0.bin.zst, chunk_1.bin.zst, etc.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Chunk metadata
#[derive(Debug, Clone)]
pub struct ChunkMetadata {
    pub total_blocks: u64,
    pub num_chunks: usize,
    pub blocks_per_chunk: u64,
    pub compression: String,
}

/// Load chunk metadata from chunks.meta file
pub fn load_chunk_metadata(chunks_dir: &Path) -> Result<Option<ChunkMetadata>> {
    let meta_file = chunks_dir.join("chunks.meta");
    if !meta_file.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&meta_file)?;
    let mut total_blocks = None;
    let mut num_chunks = None;
    let mut blocks_per_chunk = None;
    let mut compression = None;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            match key.trim() {
                "total_blocks" => total_blocks = value.trim().parse().ok(),
                "num_chunks" => num_chunks = value.trim().parse().ok(),
                "blocks_per_chunk" => blocks_per_chunk = value.trim().parse().ok(),
                "compression" => compression = Some(value.trim().to_string()),
                _ => {}
            }
        }
    }

    if let (Some(total), Some(num), Some(per_chunk), Some(comp)) =
        (total_blocks, num_chunks, blocks_per_chunk, compression)
    {
        Ok(Some(ChunkMetadata {
            total_blocks: total,
            num_chunks: num,
            blocks_per_chunk: per_chunk,
            compression: comp,
        }))
    } else {
        Ok(None)
    }
}

/// Decompress a zstd-compressed chunk file
/// 
/// OPTIMIZATION: Returns a streaming reader instead of loading entire chunk into memory
/// This prevents OOM for large chunks (50-60GB compressed = 200GB+ uncompressed)
pub fn decompress_chunk_streaming(chunk_path: &Path) -> Result<std::process::Child> {
    use std::process::{Command, Stdio};

    // OPTIMIZATION: Use streaming decompression instead of loading entire chunk
    // This allows reading blocks one at a time without loading 200GB+ into memory
    let child = Command::new("zstd")
        .arg("-d")
        .arg("--stdout")
        .arg(chunk_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to start zstd decompression: {}", chunk_path.display()))?;

    Ok(child)
}

/// Decompress a zstd-compressed chunk file (legacy - loads entire chunk)
/// 
/// WARNING: This loads the entire chunk into memory. For large chunks (50-60GB compressed),
/// this can require 200GB+ RAM. Use decompress_chunk_streaming() instead.
#[allow(dead_code)]
pub fn decompress_chunk(chunk_path: &Path) -> Result<Vec<u8>> {
    use std::process::Command;

    // Check if zstd is available
    let output = Command::new("zstd")
        .arg("--version")
        .output()
        .context("zstd not found - install with: sudo pacman -S zstd")?;

    if !output.status.success() {
        anyhow::bail!("zstd command failed");
    }

    // Decompress chunk
    let output = Command::new("zstd")
        .arg("-d")
        .arg("--stdout")
        .arg(chunk_path)
        .output()
        .with_context(|| format!("Failed to decompress chunk: {}", chunk_path.display()))?;

    if !output.status.success() {
        anyhow::bail!(
            "zstd decompression failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(output.stdout)
}

/// Load blocks from a single chunk
pub fn load_chunk_blocks(chunk_data: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut blocks = Vec::new();
    let mut offset = 0usize;

    while offset + 4 <= chunk_data.len() {
        // Read block length (u32)
        let block_len = u32::from_le_bytes([
            chunk_data[offset],
            chunk_data[offset + 1],
            chunk_data[offset + 2],
            chunk_data[offset + 3],
        ]) as usize;
        offset += 4;

        if offset + block_len > chunk_data.len() {
            anyhow::bail!("Block extends beyond chunk data");
        }

        blocks.push(chunk_data[offset..offset + block_len].to_vec());
        offset += block_len;
    }

    Ok(blocks)
}

/// Load blocks from chunked cache
/// Returns blocks for the specified range, loading chunks as needed
pub fn load_chunked_cache(
    chunks_dir: &Path,
    start_height: Option<u64>,
    max_blocks: Option<usize>,
) -> Result<Option<Vec<Vec<u8>>>> {
    // Load metadata
    let metadata = match load_chunk_metadata(chunks_dir)? {
        Some(m) => m,
        None => {
            // No chunked cache found
            return Ok(None);
        }
    };

    println!("📂 Loading from chunked cache: {} chunks, {} total blocks", 
             metadata.num_chunks, metadata.total_blocks);

    // Determine which chunks we need
    let start_idx = start_height.unwrap_or(0) as usize;
    let end_idx = if let Some(max) = max_blocks {
        (start_idx + max).min(metadata.total_blocks as usize)
    } else {
        metadata.total_blocks as usize
    };

    let start_chunk = start_idx / metadata.blocks_per_chunk as usize;
    let end_chunk = (end_idx - 1) / metadata.blocks_per_chunk as usize;

    println!("   Loading chunks {}-{} (blocks {}-{})", 
             start_chunk, end_chunk, start_idx, end_idx);

    // OPTIMIZATION: Stream blocks from chunks instead of loading entire chunks into memory
    // For 50-60GB compressed chunks, this prevents loading 200GB+ into RAM
    let mut all_blocks = Vec::new();
    for chunk_num in start_chunk..=end_chunk.min(metadata.num_chunks - 1) {
        let chunk_file = chunks_dir.join(format!("chunk_{}.bin.zst", chunk_num));
        
        if !chunk_file.exists() {
            eprintln!("   ⚠️  Chunk {} not found: {}", chunk_num, chunk_file.display());
            continue;
        }

        println!("   📦 Streaming blocks from chunk {}...", chunk_num);
        
        // OPTIMIZATION: Stream decompression instead of loading entire chunk
        use std::io::{BufReader, Read};
        use std::process::{Command, Stdio};
        
        let mut zstd_proc = Command::new("zstd")
            .arg("-d")
            .arg("--stdout")
            .arg(&chunk_file)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to start zstd for chunk {}", chunk_num))?;
        
        let mut reader = BufReader::with_capacity(128 * 1024 * 1024, // 128MB buffer
            zstd_proc.stdout.take()
                .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdout"))?);
        
        // Read blocks one at a time (streaming)
        let mut blocks_in_chunk = 0;
        loop {
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf) {
                Ok(_) => {},
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    let _ = zstd_proc.wait(); // Clean up
                    return Err(e.into());
                }
            }
            
            let block_len = u32::from_le_bytes(len_buf) as usize;
            
            // Validate block size
            if block_len > 10 * 1024 * 1024 || block_len < 88 {
                let _ = zstd_proc.wait();
                anyhow::bail!("Invalid block size in chunk {}: {} bytes", chunk_num, block_len);
            }
            
            // Read block data
            let mut block_data = vec![0u8; block_len];
            reader.read_exact(&mut block_data)?;
            
            all_blocks.push(block_data);
            blocks_in_chunk += 1;
            
            // OPTIMIZATION: Reduce progress reporting frequency (less I/O overhead)
            if blocks_in_chunk % 25000 == 0 {
                println!("     Loaded {}/{} blocks from chunk {}...", 
                        blocks_in_chunk, metadata.blocks_per_chunk, chunk_num);
            }
        }
        
        // Wait for zstd to finish
        let status = zstd_proc.wait()?;
        if !status.success() {
            anyhow::bail!("zstd decompression failed for chunk {}", chunk_num);
        }
        
        println!("   ✅ Loaded {} blocks from chunk {}", blocks_in_chunk, chunk_num);
    }

    // Filter to requested range
    if start_idx > 0 || end_idx < all_blocks.len() {
        let filtered: Vec<_> = all_blocks.into_iter()
            .skip(start_idx)
            .take(end_idx - start_idx)
            .collect();
        Ok(Some(filtered))
    } else {
        Ok(Some(all_blocks))
    }
}

/// Get chunk directory path
pub fn get_chunks_dir() -> Option<PathBuf> {
    dirs::cache_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
        .map(|cache| cache.join("blvm-bench").join("chunks"))
}

/// Check if chunked cache exists
pub fn chunked_cache_exists() -> bool {
    if let Some(chunks_dir) = get_chunks_dir() {
        chunks_dir.exists() && chunks_dir.join("chunks.meta").exists()
    } else {
        false
    }
}
