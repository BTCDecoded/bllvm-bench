//! Validate (and optionally rebuild) the chunk cache **before** `chunk_utxo_checkpoints`.
//!
//! Ensures `chunks.index` covers the heights `ChunkedBlockIterator` needs and reports
//! how far the chain runs contiguously from genesis (or from `--start`).
//!
//! ```text
//! cargo build --release --features scan --bin prep_chunk_utxo_cache
//! prep_chunk_utxo_cache
//! prep_chunk_utxo_cache --rebuild-index   # slow: rescan chunk_*.bin.zst
//! ```

use anyhow::{bail, Context, Result};
use blvm_bench::chunk_index::{
    contiguous_chain_from, ensure_chunk_block_index, missing_chunk_bin_files,
    validate_utxo_chunk_cache_index,
};
use blvm_bench::chunked_cache::load_chunk_metadata;
use blvm_bench::kernel_diff_paths::{resolve_block_cache_root, resolve_chunks_data_dir};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "prep_chunk_utxo_cache")]
struct Args {
    #[arg(long, env = "BLOCK_CACHE_DIR")]
    block_cache_dir: Option<PathBuf>,

    /// Remove `chunks.index` and rebuild from `chunk_*.bin.zst` (expensive).
    #[arg(long, default_value_t = false)]
    rebuild_index: bool,

    /// Match `chunk_utxo_checkpoints --start`: first block height to replay.
    #[arg(long = "start", default_value_t = 0)]
    start: u64,

    /// Match `chunk_utxo_checkpoints --end` (exclusive). Omit to use `chunks.meta` `total_blocks`.
    #[arg(long = "end")]
    end: Option<u64>,

    /// Minimum contiguous indexed heights starting at `--start` required to exit 0.
    #[arg(long, default_value_t = 100)]
    min_contiguous: u64,

    /// Print report but always exit 0 (do not fail the prep check).
    #[arg(long, default_value_t = false)]
    warn_only: bool,

    /// Allow missing `chunk_i.bin.zst` files (warn only; default is to fail fast).
    #[arg(long, default_value_t = false)]
    allow_missing_chunk_files: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let cache_root = resolve_block_cache_root(args.block_cache_dir.clone())?;
    let chunks_dir = resolve_chunks_data_dir(&cache_root)?;

    let metadata = load_chunk_metadata(&chunks_dir)?.with_context(|| {
        format!(
            "missing or invalid chunks.meta under {}",
            chunks_dir.display()
        )
    })?;

    let missing_files = missing_chunk_bin_files(&chunks_dir, metadata.num_chunks);
    if !missing_files.is_empty() {
        eprintln!(
            "   ⚠️  Missing chunk file(s) for metadata num_chunks={}: {:?} (expected chunk_<i>.bin.zst under {})",
            metadata.num_chunks,
            missing_files,
            chunks_dir.display()
        );
        if !args.allow_missing_chunk_files {
            bail!("cannot satisfy chunks.meta: add missing chunk files or fix num_chunks / paths");
        }
    }

    let index = ensure_chunk_block_index(&chunks_dir, args.rebuild_index)?;

    let end_cap = args
        .end
        .unwrap_or(metadata.total_blocks)
        .min(metadata.total_blocks);

    let validate_res =
        validate_utxo_chunk_cache_index(&index, args.start, end_cap, args.min_contiguous);

    let (contig, first_gap) = contiguous_chain_from(&index, args.start);
    eprintln!("   chunks dir: {}", chunks_dir.display());
    eprintln!(
        "   indexed heights: {} contiguous from {} (first gap at height {})",
        contig, args.start, first_gap
    );
    eprintln!(
        "   metadata: total_blocks={}, num_chunks={}",
        metadata.total_blocks, metadata.num_chunks
    );

    match validate_res {
        Ok(()) => {}
        Err(e) => {
            if args.warn_only {
                eprintln!("   ⚠️  WARNING: {:#}", e);
            } else {
                return Err(e);
            }
        }
    }

    eprintln!("   done; ready for chunk_utxo_checkpoints when chain data is complete.");
    Ok(())
}
