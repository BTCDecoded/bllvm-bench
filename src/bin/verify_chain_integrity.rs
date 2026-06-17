//! Verify chain integrity: genesis hash + full prev_hash chain.
//!
//! Ensures blocks at each height are the correct Bitcoin mainnet blocks.
//!
//! Usage:
//!   BLOCK_CACHE_DIR=/path ./target/release/verify_chain_integrity
//!   BLOCK_CACHE_DIR=/path ./target/release/verify_chain_integrity --limit 10000

use anyhow::{Context, Result};
use blvm_bench::chunked_cache::{ChunkedBlockIterator, load_chunk_metadata};
use blvm_protocol::constants::GENESIS_BLOCK_HASH;
use clap::Parser;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Instant;

fn block_hash_from_header(header: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(header);
    let second = Sha256::digest(&first);
    let mut out = [0u8; 32];
    out.copy_from_slice(second.as_slice());
    out.reverse();
    out
}

fn prev_hash_from_header(header: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&header[4..36]);
    out.reverse();
    out
}

#[derive(Parser, Debug)]
#[command(name = "verify_chain_integrity")]
#[command(about = "Verify genesis + prev_hash chain for chunked blockchain")]
struct Args {
    /// Limit verification to first N blocks (0 = all)
    #[arg(long, default_value = "0")]
    limit: u64,

    /// Progress interval (blocks)
    #[arg(long, default_value = "50000")]
    progress: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let chunks_dir = blvm_bench::require_block_cache_dir()?;

    let metadata = load_chunk_metadata(&chunks_dir)?
        .ok_or_else(|| anyhow::anyhow!("No chunk metadata (chunks.meta)"))?;

    let max_blocks = if args.limit > 0 {
        args.limit.min(metadata.total_blocks)
    } else {
        metadata.total_blocks
    };

    eprintln!("🔍 Chain Integrity Verification");
    eprintln!("   Chunks: {}", chunks_dir.display());
    eprintln!("   Blocks: 0..{} ({} total)", max_blocks - 1, max_blocks);
    if args.limit > 0 {
        eprintln!("   (limited to first {} blocks)", args.limit);
    }
    eprintln!();

    let mut iter = ChunkedBlockIterator::new(&chunks_dir, Some(0), Some(max_blocks as usize))?
        .ok_or_else(|| anyhow::anyhow!("Failed to create block iterator"))?;

    let mut prev_hash = [0u8; 32]; // Genesis has all-zero prev
    let mut verified = 0u64;
    let start = Instant::now();

    for height in 0..max_blocks {
        let data = match iter.next_block()? {
            Some(d) => d,
            None => anyhow::bail!("Unexpected EOF at height {}", height),
        };

        if data.len() < 80 {
            anyhow::bail!("Block {} too short ({} bytes)", height, data.len());
        }

        let header = &data[0..80];
        let block_hash = block_hash_from_header(header);
        let block_prev_hash = prev_hash_from_header(header);

        // Genesis check
        if height == 0 {
            if block_hash != GENESIS_BLOCK_HASH {
                eprintln!("❌ Genesis hash mismatch!");
                eprintln!("   Expected: {}", hex::encode(GENESIS_BLOCK_HASH));
                eprintln!("   Got:      {}", hex::encode(block_hash));
                anyhow::bail!("Genesis block is not Bitcoin mainnet genesis");
            }
            if block_prev_hash != [0u8; 32] {
                anyhow::bail!("Genesis prev_hash should be all zeros");
            }
        } else {
            // Prev hash chain
            if block_prev_hash != prev_hash {
                eprintln!("❌ Prev hash chain broken at height {}!", height);
                eprintln!("   Expected (prev block hash): {}", hex::encode(prev_hash));
                eprintln!(
                    "   Got (block's prev_hash):    {}",
                    hex::encode(block_prev_hash)
                );
                anyhow::bail!("Chain integrity failed at height {}", height);
            }
        }

        prev_hash = block_hash;
        verified += 1;

        if verified > 0 && verified % args.progress == 0 {
            let rate = verified as f64 / start.elapsed().as_secs_f64();
            eprintln!("   {} blocks verified ({:.1} blk/s)", verified, rate);
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    eprintln!();
    eprintln!("✅ Chain integrity PASSED");
    eprintln!(
        "   Verified {} blocks in {:.1}s ({:.1} blk/s)",
        verified,
        elapsed,
        verified as f64 / elapsed
    );
    eprintln!("   Genesis: correct");
    eprintln!("   Prev_hash chain: intact");

    Ok(())
}
