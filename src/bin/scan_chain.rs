//! Blockchain Scanner
//!
//! Scans blockchain blocks from the chunked cache (XOR-packaged / reordered-blk format) to measure:
//! - Output/witness size rule impact (default: BIP-110 limits)
//! - Transaction type breakdown (monetary vs Ordinals vs presigned money)
//!
//! Usage:
//!   BLOCK_CACHE_DIR=/path/to/blockchain cargo run --bin scan_chain --features scan
//!   BLOCK_CACHE_DIR=/path cargo run --bin scan_chain --features scan -- --start 800000 --end 850000
//!
//! Requires `BLOCK_CACHE_DIR` (or a populated `~/.cache/blvm-bench/chunks` fallback via `get_chunks_dir`).

use anyhow::{Context, Result};
use blvm_bench::chain_scan::{
    ChainScanResults, INSCRIPTIONS_START_HEIGHT, SEGWIT_START_HEIGHT, TAPROOT_START_HEIGHT,
    analyze_block, analyze_block_with_outpoint_index, merge_block_into_results,
};
use blvm_bench::chunked_cache::{ChunkedBlockIterator, get_chunks_dir, load_chunk_metadata};
use blvm_protocol::serialization::block::deserialize_block_with_witnesses;
use blvm_protocol::spam_filter::{SpamFilter, SpamFilterPreset};
use blvm_protocol::types::OutPoint;
use clap::Parser;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(name = "scan_chain")]
#[command(about = "Scan blockchain for output size rule impact (e.g. BIP-110)")]
struct Args {
    /// Start height (inclusive)
    #[arg(long, default_value = "800000")]
    start: u64,

    /// End height (inclusive). Use 0 for "all available"
    #[arg(long, default_value = "801000")]
    end: u64,

    /// Output JSON results to file
    #[arg(long)]
    json: Option<PathBuf>,

    /// Progress interval (blocks)
    #[arg(long, default_value = "1000")]
    progress: usize,

    /// Spam filter preset for cross-reference analysis (conservative|moderate|aggressive|strict|disabled). Default: moderate (fair balance).
    #[arg(long, default_value = "moderate")]
    spam_preset: SpamPresetArg,

    /// Block batch size for parallel processing. 1 = sequential. 64–256 typical for multi-core.
    #[arg(long, default_value = "64")]
    batch_size: usize,

    /// Track grandfathered vs unspendable for Tapscript OP_IF (requires sequential scan, slower)
    #[arg(long)]
    grandfathered: bool,

    /// BIP-110 activation height for grandfathered analysis. Prevouts created before this are grandfathered.
    /// Default ~960000 (approx Aug 2026 per bip110.org deployment timeline).
    #[arg(long, default_value = "960000")]
    bip110_activation_height: u64,
}

#[derive(Debug, Clone, Copy)]
enum SpamPresetArg {
    Conservative,
    Moderate,
    Aggressive,
    StrictInscriptions,
    Disabled,
}

impl std::str::FromStr for SpamPresetArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "conservative" => Ok(SpamPresetArg::Conservative),
            "moderate" => Ok(SpamPresetArg::Moderate),
            "aggressive" => Ok(SpamPresetArg::Aggressive),
            "strict" | "strictinscriptions" => Ok(SpamPresetArg::StrictInscriptions),
            "disabled" | "none" | "off" => Ok(SpamPresetArg::Disabled),
            _ => Err(format!(
                "Unknown preset: {}. Use conservative, moderate, aggressive, strict, or disabled.",
                s
            )),
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let chunks_dir = get_chunks_dir()
        .filter(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!(
            "Chunks directory not found. Set BLOCK_CACHE_DIR to your chunk cache root (see blvm-bench/.env.example)."
        ))?;

    if chunks_dir.join("chunks.meta").exists() {
        if let Ok(Some(meta)) = load_chunk_metadata(&chunks_dir) {
            eprintln!(
                "📦 Chunks: {} files, ~{} blocks",
                meta.num_chunks, meta.total_blocks
            );
        }
    }

    let end_height = if args.end == 0 {
        load_chunk_metadata(&chunks_dir)
            .ok()
            .flatten()
            .map(|m| m.total_blocks)
            .unwrap_or(args.start + 10000)
    } else {
        args.end
    };

    let spam_filter: Option<SpamFilter> = match args.spam_preset {
        SpamPresetArg::Disabled => None,
        p => {
            let preset = match p {
                SpamPresetArg::Conservative => SpamFilterPreset::Conservative,
                SpamPresetArg::Moderate => SpamFilterPreset::Moderate,
                SpamPresetArg::Aggressive => SpamFilterPreset::Aggressive,
                SpamPresetArg::StrictInscriptions => SpamFilterPreset::StrictInscriptions,
                SpamPresetArg::Disabled => unreachable!(),
            };
            Some(SpamFilter::with_preset(preset))
        }
    };

    eprintln!(
        "🔍 Chain Scan: blocks {} to {} (output size rules, default BIP-110)",
        args.start, end_height
    );
    eprintln!(
        "   Spam filter: {:?} (cross-reference analysis)",
        args.spam_preset
    );
    let batch_size = if args.grandfathered {
        eprintln!(
            "   Grandfathered: enabled (sequential scan, BIP-110 activation @ {})",
            args.bip110_activation_height
        );
        1usize
    } else {
        eprintln!(
            "   Batch size: {} (parallel block processing)",
            args.batch_size
        );
        args.batch_size
    };
    eprintln!("   Chunks: {}", chunks_dir.display());
    eprintln!();

    let max_blocks = (end_height - args.start + 1) as usize;
    let mut block_iter =
        ChunkedBlockIterator::new(&chunks_dir, Some(args.start), Some(max_blocks))?
            .ok_or_else(|| anyhow::anyhow!("Failed to create block iterator"))?;

    let mut results = ChainScanResults::default();
    let mut blocks_ok = 0u64;
    let mut blocks_fail = 0u64;
    let mut current_height = args.start;
    let start_time = Instant::now();
    let mut batch: Vec<(u64, Vec<u8>)> = Vec::with_capacity(batch_size);
    let mut outpoint_index: FxHashMap<OutPoint, u32> = FxHashMap::default();

    let process_batch = |batch: &[(u64, Vec<u8>)], spam_filter: Option<&SpamFilter>| {
        batch
            .par_iter()
            .map(|(height, data)| {
                deserialize_block_with_witnesses(data)
                    .map(|(block, witnesses)| {
                        Some(analyze_block(&block, &witnesses, *height, spam_filter))
                    })
                    .unwrap_or_else(|e| {
                        eprintln!("⚠️  Block {} failed to parse: {}", height, e);
                        None
                    })
            })
            .collect::<Vec<_>>()
    };

    if args.grandfathered {
        // Sequential processing with outpoint index
        loop {
            match block_iter.next_block()? {
                Some(data) => {
                    if let Ok((block, witnesses)) = deserialize_block_with_witnesses(&data) {
                        let stats = analyze_block_with_outpoint_index(
                            &block,
                            &witnesses,
                            current_height,
                            spam_filter.as_ref(),
                            &mut outpoint_index,
                            args.bip110_activation_height,
                        );
                        merge_block_into_results(&mut results, &stats);
                        blocks_ok += 1;
                    } else {
                        eprintln!("⚠️  Block {} failed to parse", current_height);
                        blocks_fail += 1;
                    }
                    current_height += 1;
                }
                None => break,
            }
            let total = blocks_ok + blocks_fail;
            if total > 0 && total % args.progress as u64 == 0 {
                let elapsed = start_time.elapsed().as_secs_f64();
                let rate = total as f64 / elapsed;
                eprintln!("   {} blocks ({:.1} blk/s)", total, rate);
            }
        }
    } else {
        loop {
            match block_iter.next_block()? {
                Some(data) => {
                    batch.push((current_height, data));
                    current_height += 1;
                }
                None => break,
            }

            if batch.len() >= batch_size {
                let stats_list = process_batch(&batch, spam_filter.as_ref());
                for stats in stats_list {
                    if let Some(s) = stats {
                        merge_block_into_results(&mut results, &s);
                        blocks_ok += 1;
                    } else {
                        blocks_fail += 1;
                    }
                }
                batch.clear();

                let total = blocks_ok + blocks_fail;
                if total > 0 && total % args.progress as u64 == 0 {
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let rate = total as f64 / elapsed;
                    eprintln!("   {} blocks ({:.1} blk/s)", total, rate);
                }
            }
        }

        if !batch.is_empty() {
            let stats_list = process_batch(&batch, spam_filter.as_ref());
            for stats in stats_list {
                if let Some(s) = stats {
                    merge_block_into_results(&mut results, &s);
                    blocks_ok += 1;
                } else {
                    blocks_fail += 1;
                }
            }
        }
    }

    let elapsed = start_time.elapsed().as_secs_f64();
    eprintln!();
    eprintln!("✅ Scan complete");

    // Print results
    eprintln!();
    eprintln!("═══════════════════════════════════════════════════════════════");
    eprintln!("CHAIN SCAN RESULTS (output size rules, BIP-110 limits)");
    eprintln!("═══════════════════════════════════════════════════════════════");
    eprintln!("Blocks scanned: {}", results.blocks_scanned);
    eprintln!("Total transactions: {}", results.total_txs);
    eprintln!(
        "Transactions that would be BLOCKED: {}",
        results.blocked_txs
    );
    if results.total_weight > 0 {
        let pct_tx = 100.0 * results.blocked_txs as f64 / results.total_txs as f64;
        let pct_wt = 100.0 * results.blocked_weight as f64 / results.total_weight as f64;
        eprintln!(
            "  Full chain: {:.2}% txs blocked, {:.2}% weight blocked",
            pct_tx, pct_wt
        );
    }
    if results.total_txs_post_inscriptions > 0 {
        let pct_tx = 100.0 * results.blocked_txs_post_inscriptions as f64
            / results.total_txs_post_inscriptions as f64;
        let pct_wt = if results.total_weight_post_inscriptions > 0 {
            100.0 * results.blocked_weight_post_inscriptions as f64
                / results.total_weight_post_inscriptions as f64
        } else {
            0.0
        };
        eprintln!(
            "  Post-inscriptions (≥{}): {:.2}% txs blocked, {:.2}% weight blocked",
            INSCRIPTIONS_START_HEIGHT, pct_tx, pct_wt
        );
    }
    if results.era_pre_segwit.blocks > 0
        || results.era_segwit.blocks > 0
        || results.era_taproot.blocks > 0
        || results.era_inscriptions.blocks > 0
    {
        eprintln!();
        eprintln!("Era breakdown:");
        for (name, era) in [
            (
                format!("Pre-SegWit (<{SEGWIT_START_HEIGHT})"),
                &results.era_pre_segwit,
            ),
            (
                format!(
                    "SegWit ({}-{})",
                    SEGWIT_START_HEIGHT,
                    TAPROOT_START_HEIGHT - 1
                ),
                &results.era_segwit,
            ),
            (
                format!(
                    "Taproot ({}-{})",
                    TAPROOT_START_HEIGHT,
                    INSCRIPTIONS_START_HEIGHT - 1
                ),
                &results.era_taproot,
            ),
            (
                format!("Inscriptions (≥{INSCRIPTIONS_START_HEIGHT})"),
                &results.era_inscriptions,
            ),
        ] {
            if era.blocks > 0 {
                let pct_tx = if era.total_txs > 0 {
                    100.0 * era.blocked_txs as f64 / era.total_txs as f64
                } else {
                    0.0
                };
                let pct_wt = if era.total_weight > 0 {
                    100.0 * era.blocked_weight as f64 / era.total_weight as f64
                } else {
                    0.0
                };
                eprintln!(
                    "  {}: {} blks, {} txs, {} blocked ({:.2}% txs, {:.2}% wt)",
                    name, era.blocks, era.total_txs, era.blocked_txs, pct_tx, pct_wt
                );
            }
        }
    }
    if !results.witness_element_histogram.is_empty() {
        eprintln!();
        eprintln!("Witness element size histogram:");
        let mut buckets: Vec<_> = results.witness_element_histogram.keys().collect();
        buckets.sort();
        for k in buckets {
            eprintln!("  {}: {}", k, results.witness_element_histogram[k]);
        }
    }
    eprintln!();
    eprintln!("Blocked by violation type (txs / weight):");
    eprintln!(
        "  Output: {} / {}",
        results.blocked_txs_with_output_violation, results.blocked_weight_with_output_violation
    );
    eprintln!(
        "  Witness: {} / {}",
        results.blocked_txs_with_witness_violation, results.blocked_weight_with_witness_violation
    );
    eprintln!(
        "  Control: {} / {}",
        results.blocked_txs_with_control_violation, results.blocked_weight_with_control_violation
    );
    eprintln!();
    eprintln!(
        "Tapscript OP_IF (BIP-110): {} txs with OP_IF/OP_NOTIF in tapscript",
        results.block_txs_with_tapscript_op_if_violation
    );
    if results.tapscript_op_if_grandfathered > 0 || results.tapscript_op_if_unspendable > 0 {
        let total = results.tapscript_op_if_grandfathered + results.tapscript_op_if_unspendable;
        eprintln!(
            "  Grandfathered (prevout created before activation): {} inputs",
            results.tapscript_op_if_grandfathered
        );
        eprintln!(
            "  Unspendable (prevout at/after activation): {} inputs",
            results.tapscript_op_if_unspendable
        );
        if total > 0 {
            let pct_g = 100.0 * results.tapscript_op_if_grandfathered as f64 / total as f64;
            eprintln!(
                "  → {:.1}% grandfathered, {:.1}% would be stuck",
                pct_g,
                100.0 - pct_g
            );
        }
    }
    eprintln!();
    eprintln!("Violations by type:");
    for (k, v) in &results.violations_by_type {
        eprintln!("  {}: {}", k, v);
    }
    if !results.collateral_violations_by_type.is_empty() {
        eprintln!();
        eprintln!("Collateral damage violations (rule-blocked, not spam):");
        for (k, v) in &results.collateral_violations_by_type {
            eprintln!("  {}: {}", k, v);
        }
    }
    eprintln!();
    eprintln!("Transaction classifications:");
    for (k, v) in &results.classifications {
        eprintln!("  {}: {}", k, v);
    }
    let ordinals_like = results
        .classifications
        .get("OrdinalsLike")
        .copied()
        .unwrap_or(0);
    let brc20_like = results
        .classifications
        .get("Brc20Like")
        .copied()
        .unwrap_or(0);
    let strict_inscriptions = ordinals_like + brc20_like;
    if strict_inscriptions > 0 && results.total_txs > 0 {
        eprintln!(
            "  → Strict inscriptions (OrdinalsLike+Brc20Like): {} ({:.2}%)",
            strict_inscriptions,
            100.0 * strict_inscriptions as f64 / results.total_txs as f64
        );
    }
    if results.spam_txs > 0 {
        eprintln!();
        eprintln!("Spam cross-reference:");
        eprintln!("  Spam txs: {}", results.spam_txs);
        eprintln!("  Spam ∩ rule-blocked: {}", results.spam_and_rule_blocked);
        eprintln!(
            "  Spam ∩ ¬rule-blocked: {}",
            results.spam_and_not_rule_blocked
        );
        eprintln!(
            "  ¬Spam ∩ rule-blocked (collateral): {}",
            results.rule_blocked_and_not_spam
        );
        eprintln!("  Spam by type:");
        for (k, v) in &results.spam_by_type {
            eprintln!("    {}: {}", k, v);
        }
        if !results.spam_by_confidence.is_empty() {
            eprintln!("  Spam by confidence:");
            for (k, v) in &results.spam_by_confidence {
                eprintln!("    {}: {}", k, v);
            }
        }
    }
    eprintln!();
    eprintln!(
        "Elapsed: {:.1}s ({:.1} blk/s)",
        elapsed,
        results.blocks_scanned as f64 / elapsed.max(0.001)
    );
    eprintln!("═══════════════════════════════════════════════════════════════");

    if let Some(ref json_path) = args.json {
        let json = serde_json::to_string_pretty(&results).context("Failed to serialize results")?;
        std::fs::write(json_path, json)
            .with_context(|| format!("Failed to write {}", json_path.display()))?;
        eprintln!("📄 Results written to {}", json_path.display());
    }

    Ok(())
}
