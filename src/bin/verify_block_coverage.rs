//! Verify that block chunks provide complete coverage of the blockchain.
//!
//! Checks:
//! - chunks.meta total_blocks vs index entries
//! - All heights 0..total_blocks-1 present (no gaps)
//! - Chunk files exist for all referenced chunks
//! - Optional: cross-check with scan_merged.json blocks_scanned

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn main() -> Result<()> {
    let chunks_dir = blvm_bench::require_block_cache_dir()?
        .to_string_lossy()
        .into_owned();
    let chunks_dir = PathBuf::from(&chunks_dir);

    println!("🔍 Block Coverage Verification");
    println!("   Chunks directory: {}", chunks_dir.display());
    println!();

    // 1. Load metadata
    let metadata = blvm_bench::chunked_cache::load_chunk_metadata(&chunks_dir)?
        .ok_or_else(|| anyhow::anyhow!("No chunk metadata found (chunks.meta)"))?;

    println!("📋 Metadata (chunks.meta):");
    println!("   total_blocks: {}", metadata.total_blocks);
    println!("   num_chunks: {}", metadata.num_chunks);
    println!("   blocks_per_chunk: {}", metadata.blocks_per_chunk);
    println!();

    // 2. Verify chunk files exist
    let mut missing_chunks = Vec::new();
    for i in 0..metadata.num_chunks {
        let chunk_file = chunks_dir.join(format!("chunk_{}.bin.zst", i));
        if !chunk_file.exists() {
            missing_chunks.push(i);
        }
    }
    if !missing_chunks.is_empty() {
        anyhow::bail!(
            "Missing chunk files: {:?}. Expected chunks 0..{}",
            missing_chunks,
            metadata.num_chunks - 1
        );
    }
    println!(
        "✅ All {} chunk files present (chunk_0..chunk_{})",
        metadata.num_chunks,
        metadata.num_chunks - 1
    );
    println!();

    // 3. Load block index
    let index_path = chunks_dir.join("chunks.index");
    if !index_path.exists() {
        anyhow::bail!(
            "Block index not found at {}. Run build_block_index first.",
            index_path.display()
        );
    }

    let index_data = std::fs::read(&index_path)
        .with_context(|| format!("Failed to read index: {}", index_path.display()))?;
    let index: blvm_bench::chunk_index::BlockIndex =
        bincode::deserialize(&index_data).context("Failed to deserialize block index")?;

    println!("📋 Block index (chunks.index):");
    println!("   entries: {}", index.len());
    println!();

    // 4. Check coverage: index.len() vs total_blocks
    let expected = metadata.total_blocks as usize;
    if index.len() != expected {
        anyhow::bail!(
            "Index entry count mismatch: index has {} entries, metadata expects {}",
            index.len(),
            expected
        );
    }
    println!(
        "✅ Index entry count matches metadata ({} blocks)",
        expected
    );

    // 5. Check for gaps: every height 0..total_blocks-1 must exist
    let heights: BTreeSet<u64> = index.keys().copied().collect();
    let mut gaps = Vec::new();
    for h in 0..metadata.total_blocks {
        if !heights.contains(&h) {
            gaps.push(h);
            if gaps.len() > 20 {
                gaps.push(u64::MAX); // sentinel for "and more"
                break;
            }
        }
    }
    if gaps.last() == Some(&u64::MAX) {
        gaps.pop();
        anyhow::bail!(
            "Missing heights in index (gaps): {:?} ... and more ({} total missing)",
            gaps,
            (0..metadata.total_blocks)
                .filter(|h| !heights.contains(h))
                .count()
        );
    }
    if !gaps.is_empty() {
        anyhow::bail!("Missing heights in index (gaps): {:?}", gaps);
    }
    println!(
        "✅ No gaps: all heights 0..{} present",
        metadata.total_blocks - 1
    );

    // 6. Check for blocks in chunk 999 (missing)
    let missing_chunk_999: Vec<u64> = index
        .iter()
        .filter(|(_, e)| e.chunk_number == 999)
        .map(|(h, _)| *h)
        .collect();
    if !missing_chunk_999.is_empty() {
        println!(
            "⚠️  {} blocks in chunk_missing (chunk 999): {:?}...",
            missing_chunk_999.len(),
            &missing_chunk_999[..missing_chunk_999.len().min(10)]
        );
    } else {
        println!("✅ No blocks in chunk_missing (all in regular chunks)");
    }

    // 7. Verify chunk numbers are valid
    let invalid_chunks: Vec<(u64, usize)> = index
        .iter()
        .filter(|(_, e)| e.chunk_number >= metadata.num_chunks && e.chunk_number != 999)
        .map(|(h, e)| (*h, e.chunk_number))
        .take(10)
        .collect();
    if !invalid_chunks.is_empty() {
        anyhow::bail!(
            "Invalid chunk numbers (>= num_chunks): {:?}",
            invalid_chunks
        );
    }
    println!(
        "✅ All chunk references valid (0..{} or 999)",
        metadata.num_chunks - 1
    );
    println!();

    // 8. Optional: cross-check with scan_merged.json
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let alt_paths = [
        cwd.join("bip110_results").join("scan_merged.json"),
        cwd.join("blvm-bench")
            .join("bip110_results")
            .join("scan_merged.json"),
        PathBuf::from("blvm-bench/bip110_results/scan_merged.json"),
    ];
    let mut scan_json = None;
    for p in alt_paths.into_iter() {
        if p.exists() {
            if let Ok(data) = std::fs::read_to_string(&p) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                    scan_json = Some((p, v));
                    break;
                }
            }
        }
    }
    if let Some((path, v)) = scan_json {
        if let Some(n) = v.get("blocks_scanned").and_then(|x| x.as_u64()) {
            println!("📋 Cross-check with scan_merged.json:");
            println!("   Path: {}", path.display());
            println!("   blocks_scanned: {}", n);
            if n == metadata.total_blocks {
                println!("✅ blocks_scanned matches total_blocks ({})", n);
            } else {
                println!(
                    "⚠️  Mismatch: scan has {}, metadata has {}",
                    n, metadata.total_blocks
                );
            }
        }
    } else {
        println!("ℹ️  scan_merged.json not found (optional cross-check skipped)");
    }

    println!();
    println!("✅ Block coverage verification PASSED");
    println!(
        "   Full chain coverage: heights 0..{} ({} blocks)",
        metadata.total_blocks - 1,
        metadata.total_blocks
    );

    Ok(())
}
