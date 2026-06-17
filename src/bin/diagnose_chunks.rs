//! Quick diagnostic to count blocks in each chunk

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn main() -> Result<()> {
    println!("🔍 Diagnosing chunk contents...\n");

    let chunks_dir = blvm_bench::require_block_cache_dir()?;

    // Find all chunk files
    let mut chunk_files: Vec<(usize, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&chunks_dir)? {
        let entry = entry?;
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("chunk_") && name.ends_with(".bin.zst") && !name.contains("missing")
            {
                if let Some(num_str) = name
                    .strip_prefix("chunk_")
                    .and_then(|s| s.strip_suffix(".bin.zst"))
                {
                    if let Ok(num) = num_str.parse::<usize>() {
                        chunk_files.push((num, path));
                    }
                }
            }
        }
    }
    chunk_files.sort_by_key(|(num, _)| *num);

    println!("Found {} chunk files\n", chunk_files.len());

    // Track all unique block hashes
    let mut all_hashes: HashSet<[u8; 32]> = HashSet::new();
    let mut duplicate_count = 0u64;

    for (chunk_num, chunk_path) in &chunk_files {
        let file_size = std::fs::metadata(chunk_path)?.len();
        println!(
            "📦 Chunk {}: {} compressed",
            chunk_num,
            format_size(file_size)
        );

        // Decompress and count blocks (first 10k only for speed)
        let zstd_proc = Command::new("zstd")
            .args(["-d", "-c", "-T0"])
            .arg(chunk_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdout = zstd_proc.stdout.unwrap();
        let mut reader = BufReader::with_capacity(16 * 1024 * 1024, stdout);

        let mut block_count = 0u64;
        let mut chunk_new_hashes = 0u64;
        let mut chunk_duplicates = 0u64;
        let mut first_hash: Option<[u8; 32]> = None;
        let mut prev_hash_sample: Option<[u8; 32]> = None;

        loop {
            // Read block size
            let mut size_buf = [0u8; 4];
            if reader.read_exact(&mut size_buf).is_err() {
                break;
            }
            let block_size = u32::from_le_bytes(size_buf) as usize;

            if !(80..=4_000_000).contains(&block_size) {
                break;
            }

            // Read block header
            let mut header = [0u8; 80];
            if reader.read_exact(&mut header).is_err() {
                break;
            }

            // Get prev_hash from header
            let mut prev_hash = [0u8; 32];
            prev_hash.copy_from_slice(&header[4..36]);

            if block_count == 0 {
                prev_hash_sample = Some(prev_hash);
            }

            // Calculate block hash
            let first = Sha256::digest(header);
            let second = Sha256::digest(first);
            let mut block_hash = [0u8; 32];
            block_hash.copy_from_slice(&second);
            block_hash.reverse(); // Big-endian

            if first_hash.is_none() {
                first_hash = Some(block_hash);
            }

            // Check if duplicate
            if all_hashes.contains(&block_hash) {
                chunk_duplicates += 1;
                duplicate_count += 1;
            } else {
                all_hashes.insert(block_hash);
                chunk_new_hashes += 1;
            }

            // Skip rest of block
            let remaining = block_size - 80;
            let mut skip_buf = vec![0u8; remaining.min(65536)];
            let mut skipped = 0;
            while skipped < remaining {
                let to_read = (remaining - skipped).min(65536);
                skip_buf.resize(to_read, 0);
                if reader.read_exact(&mut skip_buf).is_err() {
                    break;
                }
                skipped += to_read;
            }

            block_count += 1;

            // Stop early for large chunks (just sample)
            if block_count >= 50000 {
                break;
            }
        }

        let is_genesis = prev_hash_sample
            .map(|h| h.iter().all(|&b| b == 0))
            .unwrap_or(false);

        println!("   Blocks scanned: {block_count} (sample)");
        println!(
            "   New unique: {chunk_new_hashes}, Duplicates: {chunk_duplicates}"
        );
        if let Some(hash) = first_hash {
            println!("   First block hash: {}...", hex::encode(&hash[..8]));
        }
        println!(
            "   Starts with genesis: {}",
            if is_genesis { "YES" } else { "NO" }
        );
        println!();
    }

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📊 SUMMARY:");
    println!("   Total unique blocks found: {}", all_hashes.len());
    println!("   Total duplicates found: {duplicate_count}");

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
