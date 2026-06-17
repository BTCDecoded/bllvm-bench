//! Recollect blocks from Bitcoin Core blk*.dat files
//!
//! This reads blocks directly from the local copy of XOR-packaged encrypted block files,
//! applies XOR decryption, chains by prev_hash to determine height, and stores in chunks.

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

const BLOCK_MAGIC: [u8; 4] = [0xf9, 0xbe, 0xb4, 0xd9];
const BLOCKS_PER_CHUNK: usize = 125_000;

// XOR encryption keys (alternating every 4 bytes)
const XOR_KEY1: [u8; 4] = [0x84, 0x22, 0xe9, 0xad];
const XOR_KEY2: [u8; 4] = [0xb7, 0x8f, 0xff, 0x14];

/// Decrypt bytes using the XOR scheme used by packaged blk trees
fn xor_decrypt(data: &mut [u8], file_offset: u64) {
    let key1_u32 = u32::from_le_bytes(XOR_KEY1);
    let key2_u32 = u32::from_le_bytes(XOR_KEY2);

    let mut offset = file_offset;
    let mut i = 0;

    // Handle unaligned start
    while i < data.len() && offset % 4 != 0 {
        let key = if (offset / 4) % 2 == 0 {
            &XOR_KEY1
        } else {
            &XOR_KEY2
        };
        data[i] ^= key[(offset % 4) as usize];
        offset += 1;
        i += 1;
    }

    // Process aligned 4-byte chunks (fast path)
    while i + 4 <= data.len() {
        let key_u32 = if (offset / 4) % 2 == 0 {
            key1_u32
        } else {
            key2_u32
        };
        let chunk = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let decrypted = chunk ^ key_u32;
        data[i..i + 4].copy_from_slice(&decrypted.to_le_bytes());
        offset += 4;
        i += 4;
    }

    // Handle remaining bytes
    while i < data.len() {
        let key = if (offset / 4) % 2 == 0 {
            &XOR_KEY1
        } else {
            &XOR_KEY2
        };
        data[i] ^= key[(offset % 4) as usize];
        offset += 1;
        i += 1;
    }
}

fn main() -> Result<()> {
    let blocks_dir = blvm_bench::block_cache_env::require_bitcoin_blk_dir()?;
    let chunks_dir = blvm_bench::require_block_cache_dir()?;

    println!("🔨 Recollecting blocks from Bitcoin Core data (XOR encrypted)...");
    println!("   Source: {}", blocks_dir.display());
    println!("   Target: {}", chunks_dir.display());

    if !blocks_dir.exists() {
        anyhow::bail!("Bitcoin blk files not found at {}.", blocks_dir.display());
    }

    // Find all blk files
    let mut blk_files: Vec<PathBuf> = std::fs::read_dir(&blocks_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("blk") && n.ends_with(".dat"))
                .unwrap_or(false)
        })
        .collect();
    blk_files.sort();

    println!("   Found {} blk files", blk_files.len());

    // Phase 1: Scan all blocks and build hash map
    println!("\n📖 Phase 1: Scanning all blocks to build chain map...");
    println!("   Reading block headers with XOR decryption...");

    // Map: prev_hash -> (block_hash, file_path, offset, size)
    let mut blocks_by_prev: HashMap<[u8; 32], ([u8; 32], PathBuf, u64, u32)> = HashMap::new();
    let mut genesis_block: Option<([u8; 32], PathBuf, u64, u32)> = None;
    let mut total_blocks = 0u64;

    for (file_idx, blk_file) in blk_files.iter().enumerate() {
        if file_idx % 100 == 0 {
            println!(
                "   Scanning file {}/{} ({} blocks found)...",
                file_idx,
                blk_files.len(),
                total_blocks
            );
        }

        let file = std::fs::File::open(blk_file)?;
        let file_len = file.metadata()?.len();
        let mut reader = BufReader::with_capacity(1024 * 1024, file); // 1MB buffer for local files

        let mut pos: u64 = 0;
        while pos < file_len.saturating_sub(8) {
            // Read and decrypt magic and size (8 bytes)
            let mut header = [0u8; 8];
            if reader.read_exact(&mut header).is_err() {
                break;
            }

            // Decrypt the header
            xor_decrypt(&mut header, pos);

            if header[0..4] != BLOCK_MAGIC {
                // Seek forward 1 byte and try again
                if reader.seek(SeekFrom::Current(-7)).is_err() {
                    break;
                }
                pos += 1;
                continue;
            }

            let block_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
            if !(80..=4_000_000).contains(&block_size) {
                pos += 8;
                continue;
            }

            let block_offset = pos + 8; // Offset of actual block data (after magic+size)

            // Read and decrypt block header (80 bytes)
            let mut block_header = [0u8; 80];
            if reader.read_exact(&mut block_header).is_err() {
                break;
            }
            xor_decrypt(&mut block_header, block_offset);

            // Calculate block hash
            let first_hash = Sha256::digest(block_header);
            let second_hash = Sha256::digest(first_hash);
            let mut block_hash = [0u8; 32];
            block_hash.copy_from_slice(&second_hash);
            block_hash.reverse(); // Big-endian

            // Get prev_hash
            let mut prev_hash = [0u8; 32];
            prev_hash.copy_from_slice(&block_header[4..36]);
            prev_hash.reverse(); // Big-endian

            // Check if genesis
            let is_genesis = block_header[4..36].iter().all(|&b| b == 0);
            if is_genesis {
                if genesis_block.is_none() {
                    genesis_block = Some((block_hash, blk_file.clone(), block_offset, block_size));
                    println!(
                        "   Found genesis block: {}...",
                        hex::encode(&block_hash[..8])
                    );
                }
            } else {
                blocks_by_prev.insert(
                    prev_hash,
                    (block_hash, blk_file.clone(), block_offset, block_size),
                );
            }

            // Skip rest of block
            let remaining = block_size as i64 - 80;
            if remaining > 0
                && reader.seek(SeekFrom::Current(remaining)).is_err() {
                    break;
                }

            pos += 8 + block_size as u64;
            total_blocks += 1;
        }
    }

    println!(
        "   ✅ Scanned {} blocks, {} unique by prev_hash",
        total_blocks,
        blocks_by_prev.len()
    );

    let genesis = genesis_block.ok_or_else(|| anyhow::anyhow!("Genesis block not found!"))?;

    // Phase 2: Chain blocks by prev_hash to determine order
    println!("\n🔗 Phase 2: Chaining blocks by prev_hash...");

    // Vec of (height, block_hash, file_path, offset, size)
    let mut chain: Vec<(u64, [u8; 32], PathBuf, u64, u32)> = Vec::new();

    // Start with genesis
    chain.push((0, genesis.0, genesis.1, genesis.2, genesis.3));
    let mut current_hash = genesis.0;
    let mut height = 1u64;

    loop {
        if let Some((block_hash, file_path, offset, size)) = blocks_by_prev.remove(&current_hash) {
            chain.push((height, block_hash, file_path, offset, size));
            current_hash = block_hash;
            height += 1;

            if height % 50000 == 0 {
                println!("   Chained {height} blocks...");
            }
        } else {
            // No more blocks
            break;
        }
    }

    println!(
        "   ✅ Chained {} blocks (0 to {})",
        chain.len(),
        chain.len() - 1
    );

    if chain.len() < 250_000 {
        println!(
            "   ⚠️  Warning: Only {} blocks in chain, expected 250k+",
            chain.len()
        );
    }

    // Phase 3: Read blocks in order and write to chunks
    println!("\n📦 Phase 3: Writing blocks to chunks...");

    // Don't backup old chunks - just overwrite

    // Process blocks in chunks
    let num_chunks = chain.len().div_ceil(BLOCKS_PER_CHUNK);
    println!(
        "   Will create {num_chunks} chunks of {BLOCKS_PER_CHUNK} blocks each"
    );

    // Open file handles cache to avoid reopening
    let mut file_cache: HashMap<PathBuf, std::fs::File> = HashMap::new();

    for chunk_idx in 0..num_chunks {
        let start_height = chunk_idx * BLOCKS_PER_CHUNK;
        let end_height = ((chunk_idx + 1) * BLOCKS_PER_CHUNK).min(chain.len());

        let chunk_path = chunks_dir.join(format!("chunk_{chunk_idx}.bin.zst"));

        // Skip if chunk already exists and is valid (at least 1MB)
        if chunk_path.exists() {
            if let Ok(metadata) = std::fs::metadata(&chunk_path) {
                if metadata.len() > 1024 * 1024 {
                    println!(
                        "   ⏭️  Skipping chunk {} (already exists, {} compressed)",
                        chunk_idx,
                        format_size(metadata.len())
                    );
                    continue;
                }
            }
        }

        println!(
            "   📦 Creating chunk {} (blocks {}-{})...",
            chunk_idx,
            start_height,
            end_height - 1
        );

        // Create temp uncompressed file
        let temp_path = chunks_dir.join(format!("chunk_{chunk_idx}.bin.tmp"));

        {
            let temp_file = std::fs::File::create(&temp_path)?;
            let mut writer = BufWriter::with_capacity(16 * 1024 * 1024, temp_file);

            for idx in start_height..end_height {
                let (_height, _block_hash, file_path, offset, size) = &chain[idx];

                // Get or open file
                let file = if let Some(f) = file_cache.get_mut(file_path) {
                    f
                } else {
                    let f = std::fs::File::open(file_path)?;
                    file_cache.insert(file_path.clone(), f);
                    file_cache.get_mut(file_path).unwrap()
                };

                // Seek to block
                file.seek(SeekFrom::Start(*offset))?;

                // Read and decrypt block data
                let mut block_data = vec![0u8; *size as usize];
                file.read_exact(&mut block_data)?;
                xor_decrypt(&mut block_data, *offset);

                // Write to chunk: [size: u32][block_data]
                writer.write_all(&(*size).to_le_bytes())?;
                writer.write_all(&block_data)?;

                if idx % 10000 == 0 && idx > start_height {
                    print!(".");
                    std::io::stdout().flush()?;
                }
            }

            writer.flush()?;
        }
        println!();

        // Compress chunk (using -3 for optimal speed/compression balance)
        println!("   🗜️  Compressing chunk {chunk_idx}...");
        let status = std::process::Command::new("zstd")
            .args(["-T0", "-3", "-f", "-o"])
            .arg(&chunk_path)
            .arg(&temp_path)
            .status()?;

        if !status.success() {
            anyhow::bail!("zstd compression failed for chunk {}", chunk_idx);
        }

        // Remove temp file
        std::fs::remove_file(&temp_path)?;

        let compressed_size = std::fs::metadata(&chunk_path)?.len();
        println!(
            "   ✅ Chunk {} created: {} blocks, {} compressed",
            chunk_idx,
            end_height - start_height,
            format_size(compressed_size)
        );
    }

    // Update metadata
    let meta_path = chunks_dir.join("chunks.meta");
    let meta_content = format!(
        "# Chunk metadata\n# Recollected from Bitcoin Core blk files (XOR decrypted)\ntotal_blocks={}\nnum_chunks={}\nblocks_per_chunk={}\ncompression=zstd\n",
        chain.len(),
        num_chunks,
        BLOCKS_PER_CHUNK
    );
    std::fs::write(&meta_path, meta_content)?;

    // Delete old hash map (will be rebuilt)
    let hashmap_path = chunks_dir.join("chunks.hashmap");
    if hashmap_path.exists() {
        std::fs::remove_file(&hashmap_path)?;
    }

    // Delete old index (will be rebuilt)
    let index_path = chunks_dir.join("chunks.index");
    if index_path.exists() {
        std::fs::remove_file(&index_path)?;
    }

    println!("\n✅ Recollection complete!");
    println!(
        "   {} blocks collected in {} chunks",
        chain.len(),
        num_chunks
    );
    println!("\n💡 Next steps:");
    println!("   1. Run build_hashmap_only to rebuild hash map");
    println!("   2. Run rebuild_index_properly to build index");

    Ok(())
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}
