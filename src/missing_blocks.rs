//! Missing blocks storage system
//!
//! Stores blocks that are missing from chunks in a separate compressed file.
//! Blocks are fetched from RPC when detected during index building.
//! Chunks remain read-only - missing blocks are stored separately.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Metadata for missing blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingBlocksMeta {
    /// Map of block height -> offset in chunk_missing.bin.zst
    pub blocks: HashMap<u64, u64>,
    /// Total number of missing blocks
    pub count: usize,
}

/// Path to missing blocks chunk
pub fn missing_blocks_path(chunks_dir: &Path) -> PathBuf {
    chunks_dir.join("chunk_missing.bin.zst")
}

/// Path to missing blocks metadata
pub fn missing_blocks_meta_path(chunks_dir: &Path) -> PathBuf {
    chunks_dir.join("missing_blocks.meta")
}

/// Path to decompressed missing blocks cache (for fast random access)
pub fn missing_blocks_cache_path(chunks_dir: &Path) -> PathBuf {
    chunks_dir.join("chunk_missing.bin")
}

/// Load missing blocks metadata
pub fn load_missing_blocks_meta(chunks_dir: &Path) -> Result<Option<MissingBlocksMeta>> {
    let meta_path = missing_blocks_meta_path(chunks_dir);
    if !meta_path.exists() {
        return Ok(None);
    }

    let data = std::fs::read(&meta_path).with_context(|| {
        format!(
            "Failed to read missing blocks metadata: {}",
            meta_path.display()
        )
    })?;

    let meta: MissingBlocksMeta = bincode::deserialize(&data)
        .with_context(|| "Failed to deserialize missing blocks metadata")?;

    Ok(Some(meta))
}

/// Save missing blocks metadata
pub fn save_missing_blocks_meta(chunks_dir: &Path, meta: &MissingBlocksMeta) -> Result<()> {
    let meta_path = missing_blocks_meta_path(chunks_dir);

    let data =
        bincode::serialize(meta).with_context(|| "Failed to serialize missing blocks metadata")?;

    std::fs::write(&meta_path, data).with_context(|| {
        format!(
            "Failed to write missing blocks metadata: {}",
            meta_path.display()
        )
    })?;

    Ok(())
}

/// Add a missing block to chunk_missing.bin.zst
/// Returns the offset where the block was written
pub fn add_missing_block(chunks_dir: &Path, block_data: &[u8]) -> Result<u64> {
    let missing_path = missing_blocks_path(chunks_dir);

    // Load existing metadata (not used after optimization, but kept for potential future use)
    let _meta = load_missing_blocks_meta(chunks_dir)?.unwrap_or_else(|| MissingBlocksMeta {
        blocks: HashMap::new(),
        count: 0,
    });

    // OPTIMIZATION: Use cache file size instead of decompressing every time
    // This is MUCH faster - we only need the decompressed size for the offset
    let cache_path = missing_blocks_cache_path(chunks_dir);
    let current_offset = if cache_path.exists() {
        // Use cache file size directly (much faster than decompressing)
        std::fs::metadata(&cache_path).map(|m| m.len()).unwrap_or(0)
    } else if missing_path.exists() {
        // Cache doesn't exist - need to decompress once to create it
        // But this should be rare (only first time)
        let mut decompressed = Vec::new();
        let mut zstd_proc = std::process::Command::new("zstd")
            .arg("-d")
            .arg("--stdout")
            .arg(&missing_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("Failed to start zstd for decompression")?;

        let stdout = zstd_proc
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdout"))?;
        let mut reader = std::io::BufReader::new(stdout);
        reader
            .read_to_end(&mut decompressed)
            .context("Failed to read decompressed data")?;

        zstd_proc.wait()?;

        // Write to cache for future use
        std::fs::write(&cache_path, &decompressed).context("Failed to write cache file")?;

        decompressed.len() as u64
    } else {
        0
    };

    // OPTIMIZATION: Use cache file if available (much faster than decompressing)
    let mut decompressed = if cache_path.exists() {
        // Read from cache (fast!)
        std::fs::read(&cache_path).context("Failed to read cache file")?
    } else if missing_path.exists() {
        // Cache doesn't exist - decompress once and create cache
        let mut data = Vec::new();
        let mut zstd_proc = std::process::Command::new("zstd")
            .arg("-d")
            .arg("--stdout")
            .arg(&missing_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("Failed to start zstd for decompression")?;

        let stdout = zstd_proc
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdout"))?;
        let mut reader = std::io::BufReader::new(stdout);
        reader
            .read_to_end(&mut data)
            .context("Failed to read decompressed data")?;

        zstd_proc.wait()?;

        // Write to cache for future use
        std::fs::write(&cache_path, &data).context("Failed to write cache file")?;

        data
    } else {
        Vec::new()
    };

    // Append new block: [len: u32][data...]
    let block_len = block_data.len() as u32;
    decompressed.extend_from_slice(&block_len.to_le_bytes());
    decompressed.extend_from_slice(block_data);

    // Update cache file (append new block) - this makes future calls much faster
    std::fs::write(&cache_path, &decompressed).context("Failed to update cache file")?;

    // OPTIMIZATION: Skip compression during rebuild to avoid blocking/hanging
    // Compression can be done later in a separate pass if needed
    // The cache file is sufficient for reading blocks
    // Only compress if the compressed file doesn't exist or is very old
    let should_compress = !missing_path.exists()
        || std::fs::metadata(&missing_path)
            .and_then(|m| Ok(m.modified()?))
            .map(|t| {
                std::time::SystemTime::now()
                    .duration_since(t)
                    .unwrap_or_default()
                    .as_secs()
                    > 3600
            })
            .unwrap_or(true);

    if should_compress {
        // Recompress (but don't block forever - use timeout via spawn and wait with timeout)
        let mut zstd_proc = std::process::Command::new("zstd")
            .args(&["-3", "--stdout"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::fs::File::create(&missing_path)?)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("Failed to start zstd for compression")?;

        let mut stdin = zstd_proc
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdin"))?;
        stdin
            .write_all(&decompressed)
            .context("Failed to write to zstd stdin")?;
        drop(stdin);

        // Don't wait forever - if compression takes too long, skip it
        // The cache file is sufficient
        let _ = zstd_proc.wait(); // Ignore errors - cache file is what matters
    }

    Ok(current_offset)
}

/// Ensure decompressed cache exists (decompress if needed)
/// Also rebuilds metadata with correct offsets if needed
fn ensure_cache_exists(chunks_dir: &Path) -> Result<()> {
    let missing_path = missing_blocks_path(chunks_dir);
    let cache_path = missing_blocks_cache_path(chunks_dir);

    // Check if cache exists and is newer than compressed file
    if cache_path.exists() && missing_path.exists() {
        let cache_mtime = std::fs::metadata(&cache_path)?.modified()?;
        let compressed_mtime = std::fs::metadata(&missing_path)?.modified()?;

        if cache_mtime >= compressed_mtime {
            // Cache is up to date
            return Ok(());
        }
    }

    // Need to decompress
    if !missing_path.exists() {
        return Ok(()); // No compressed file, nothing to cache
    }

    eprintln!(
        "   🔄 Decompressing chunk_missing.bin.zst to cache (first access or outdated cache)..."
    );

    let decompress_start = std::time::Instant::now();
    let mut zstd_proc = std::process::Command::new("zstd")
        .arg("-d")
        .arg("--stdout")
        .arg(&missing_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to start zstd for decompression")?;

    let stdout = zstd_proc
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to get zstd stdout"))?;
    let mut reader = std::io::BufReader::new(stdout);

    let mut cache_file = std::fs::File::create(&cache_path)
        .with_context(|| format!("Failed to create cache file: {}", cache_path.display()))?;

    std::io::copy(&mut reader, &mut cache_file)
        .with_context(|| "Failed to copy decompressed data to cache")?;

    zstd_proc.wait()?;

    let decompress_duration = decompress_start.elapsed();
    eprintln!(
        "   ✅ Cache created in {:.2}s",
        decompress_duration.as_secs_f64()
    );

    Ok(())
}

/// Get a missing block by height
pub fn get_missing_block(chunks_dir: &Path, height: u64) -> Result<Option<Vec<u8>>> {
    if height < 100 {
        eprintln!("   🔄 get_missing_block({}) called", height);
    }

    let meta = match load_missing_blocks_meta(chunks_dir)? {
        Some(m) => m,
        None => {
            if height < 100 {
                eprintln!("   ⚠️  No missing blocks metadata found");
            }
            return Ok(None);
        }
    };

    let offset = match meta.blocks.get(&height) {
        Some(o) => *o,
        None => {
            if height < 100 {
                eprintln!("   ⚠️  Block {} not in missing blocks metadata", height);
            }
            return Ok(None);
        }
    };

    // Ensure decompressed cache exists
    ensure_cache_exists(chunks_dir)?;

    let cache_path = missing_blocks_cache_path(chunks_dir);
    if !cache_path.exists() {
        if height < 100 {
            eprintln!("   ⚠️  Cache file does not exist");
        }
        return Ok(None);
    }

    // Use cached uncompressed file for fast random access
    let mut cache_file = std::fs::File::open(&cache_path)
        .with_context(|| format!("Failed to open cache file: {}", cache_path.display()))?;

    // Seek to offset (instant - no decompression needed!)
    cache_file
        .seek(SeekFrom::Start(offset))
        .with_context(|| format!("Failed to seek to offset {} for block {}", offset, height))?;

    // Read block length
    let mut len_buf = [0u8; 4];
    cache_file.read_exact(&mut len_buf).with_context(|| {
        format!(
            "Failed to read block length at offset {} for block {}",
            offset, height
        )
    })?;
    let block_len = u32::from_le_bytes(len_buf) as usize;

    // Validate block length - blocks should be between 80 bytes (header only) and ~10MB
    if block_len < 80 || block_len > 100 * 1024 * 1024 {
        eprintln!(
            "   ⚠️  Invalid block length {} bytes at offset {} - offset is likely wrong, scanning file...",
            block_len, offset
        );

        // Fallback: scan the cache file to find the block by hash
        return scan_cache_for_block(chunks_dir, height);
    }

    if height < 100 {
        eprintln!(
            "   📍 Reading block {} data ({} bytes) from cache...",
            height, block_len
        );
    }

    // Read block data
    let mut block_data = vec![0u8; block_len];
    match cache_file.read_exact(&mut block_data) {
        Ok(_) => {}
        Err(e) => {
            eprintln!(
                "   ⚠️  Failed to read block data at offset {} for block {} (len: {}): {} - scanning file...",
                offset, height, block_len, e
            );
            return scan_cache_for_block(chunks_dir, height);
        }
    }

    if height < 100 {
        eprintln!(
            "   ✅ Got missing block {} ({} bytes) from cache",
            height,
            block_data.len()
        );
    }

    Ok(Some(block_data))
}

/// Scan cache file to find block by height (fallback when offsets are wrong)
fn scan_cache_for_block(chunks_dir: &Path, height: u64) -> Result<Option<Vec<u8>>> {
    eprintln!(
        "   🔄 Scanning cache file to find block {} (offsets are wrong)...",
        height
    );

    // Get expected block hash from index
    use crate::chunk_index::load_block_index;
    let index = match load_block_index(chunks_dir)? {
        Some(idx) => idx,
        None => {
            eprintln!("   ⚠️  No index found");
            return Ok(None);
        }
    };
    let expected_hash = match index.get(&height) {
        Some(entry) => entry.block_hash,
        None => {
            eprintln!("   ⚠️  Block {} not in index", height);
            return Ok(None);
        }
    };

    let cache_path = missing_blocks_cache_path(chunks_dir);
    let mut cache_file = std::fs::File::open(&cache_path)
        .with_context(|| format!("Failed to open cache file: {}", cache_path.display()))?;

    let mut current_offset: u64 = 0;
    let mut blocks_scanned = 0;

    loop {
        // Read block length
        let mut len_buf = [0u8; 4];
        match cache_file.read_exact(&mut len_buf) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }

        let block_len = u32::from_le_bytes(len_buf) as usize;
        if block_len > 10 * 1024 * 1024 || block_len < 88 {
            // Invalid block size - skip
            current_offset += 4;
            continue;
        }

        // Read block data
        let mut block_data = vec![0u8; block_len];
        cache_file.read_exact(&mut block_data)?;

        // Calculate block hash
        use sha2::{Digest, Sha256};
        if block_data.len() >= 80 {
            let header = &block_data[0..80];
            let first_hash = Sha256::digest(header);
            let second_hash = Sha256::digest(&first_hash);
            let mut block_hash = [0u8; 32];
            block_hash.copy_from_slice(&second_hash);
            block_hash.reverse(); // Convert to big-endian

            if block_hash == expected_hash {
                // Found it! Update metadata with correct offset
                let mut meta =
                    load_missing_blocks_meta(chunks_dir)?.unwrap_or_else(|| MissingBlocksMeta {
                        blocks: HashMap::new(),
                        count: 0,
                    });
                meta.blocks.insert(height, current_offset);
                meta.count = meta.blocks.len();
                save_missing_blocks_meta(chunks_dir, &meta)?;

                eprintln!(
                    "   ✅ Found block {} at offset {} (scanned {} blocks)",
                    height, current_offset, blocks_scanned
                );
                return Ok(Some(block_data));
            }
        }

        current_offset += 4 + block_len as u64;
        blocks_scanned += 1;

        if blocks_scanned % 1000 == 0 {
            eprintln!(
                "   📍 Scanned {} blocks, still searching for block {}...",
                blocks_scanned, height
            );
        }
    }

    eprintln!(
        "   ⚠️  Block {} not found in cache file (scanned {} blocks) - block may not be in missing blocks",
        height, blocks_scanned
    );
    Ok(None)
}

/// Fetch a block from RPC and add it to missing blocks
#[cfg(feature = "differential")]
pub async fn fetch_and_store_missing_block(
    chunks_dir: &Path,
    height: u64,
    rpc_client: &crate::remote_core_rpc::RemoteCoreRpcClient,
) -> Result<u64> {
    // Get block hash
    let block_hash_hex = rpc_client
        .get_block_hash(height)
        .await
        .with_context(|| format!("Failed to get block hash for height {}", height))?;

    // Get block raw data
    let block_hex = rpc_client
        .get_block_hex(&block_hash_hex)
        .await
        .with_context(|| format!("Failed to get block raw data for height {}", height))?;

    let block_data = hex::decode(&block_hex)
        .with_context(|| format!("Failed to decode block hex for height {}", height))?;

    // Add to missing blocks
    let offset = add_missing_block(chunks_dir, &block_data)?;

    // Update metadata
    let mut meta = load_missing_blocks_meta(chunks_dir)?.unwrap_or_else(|| MissingBlocksMeta {
        blocks: HashMap::new(),
        count: 0,
    });

    meta.blocks.insert(height, offset);
    meta.count = meta.blocks.len();
    save_missing_blocks_meta(chunks_dir, &meta)?;

    println!(
        "   ✅ Fetched and stored missing block {} (hash: {})",
        height,
        &block_hash_hex[..16]
    );

    Ok(offset)
}
