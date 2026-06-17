//! Merge multiple chain scan result JSON files into one.
//!
//! Usage: merge_scan_results file1.json file2.json file3.json -o merged.json

#![cfg(any(feature = "differential", feature = "scan"))]

use anyhow::{Context, Result};
use blvm_bench::chain_scan::{ChainScanResults, merge_results_into};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "merge_scan_results")]
#[command(about = "Merge chain scan result JSON files")]
struct Args {
    /// Input JSON files (in order: will be merged sequentially)
    input: Vec<PathBuf>,

    /// Output merged JSON file
    #[arg(short, long)]
    output: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.input.is_empty() {
        anyhow::bail!("No input files specified");
    }

    let mut acc: ChainScanResults = ChainScanResults::default();

    for (i, path) in args.input.iter().enumerate() {
        let json = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let other: ChainScanResults = serde_json::from_str(&json)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        eprintln!(
            "  {}: {} blocks, {} txs, {} blocked",
            i + 1,
            other.blocks_scanned,
            other.total_txs,
            other.blocked_txs
        );
        merge_results_into(&mut acc, &other);
    }

    let out_json =
        serde_json::to_string_pretty(&acc).context("Failed to serialize merged results")?;
    std::fs::write(&args.output, out_json)
        .with_context(|| format!("Failed to write {}", args.output.display()))?;

    eprintln!();
    eprintln!(
        "Merged: {} blocks, {} txs, {} blocked",
        acc.blocks_scanned, acc.total_txs, acc.blocked_txs
    );
    eprintln!("Written to {}", args.output.display());

    Ok(())
}
