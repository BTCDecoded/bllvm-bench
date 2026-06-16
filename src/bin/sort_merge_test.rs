//! Sort-Merge Differential Validation CLI
//!
//! **Full-chain Phase 1 (scripts):** step 6 verifies BLVM scripts on every mainnet input.
//! Combined with `block_kernel_diff` (Phase 2, block accept/reject), this is the complete
//! mainnet differential program. See **`docs/FULL_CHAIN_DIFFERENTIAL.md`** — step 6 uses the
//! canonical chain as oracle, not per-input `bitcoinconsensus`.
//!
//! Gold-standard differential testing using external sort + merge-join.
//!
//! Usage:
//!   sort_merge_test step1    # Extract input references
//!   sort_merge_test step2    # Sort inputs by prevout
//!   sort_merge_test step3    # Extract outputs  
//!   sort_merge_test step3b   # Sort outputs by txid
//!   sort_merge_test step4    # Merge-join to get prevouts
//!   sort_merge_test step5    # Sort prevouts by spending location
//!   sort_merge_test step6    # Verify scripts in parallel
//!   sort_merge_test all      # Run all steps

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use blvm_protocol::types::Network;

use blvm_bench::sort_merge::{
    input_refs::{extract_input_refs, sort_input_refs},
    merge_join::{merge_join, sort_joined},
    output_refs::{extract_outputs, sort_outputs},
    verify::{smoke_test_prevout_reader, verify_scripts},
};

// Configuration from environment
fn get_env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    let block_cache_dir = blvm_bench::require_block_cache_dir()?;
    let data_dir = std::env::var("SORT_MERGE_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| block_cache_dir.join("sort_merge_data"));
    let start_height: u64 = get_env("START_HEIGHT", "0").parse()?;
    let end_height: u64 = get_env("END_HEIGHT", "912723").parse()?;
    let progress_interval: u64 = get_env("PROGRESS_INTERVAL", "10000").parse()?;

    // Ensure data directory exists
    std::fs::create_dir_all(&data_dir)?;

    // File paths
    let inputs_unsorted = data_dir.join("inputs_unsorted.bin");
    let inputs_sorted = data_dir.join("inputs_sorted.bin");
    let outputs_unsorted = data_dir.join("outputs_unsorted.bin");
    let outputs_sorted = data_dir.join("outputs_sorted.bin");
    let joined_unsorted = data_dir.join("joined_unsorted.bin");
    let joined_sorted = data_dir.join("joined_sorted.bin");

    // The block cache dir contains chunks.meta and chunk_*.bin.zst files
    let chunks_dir = &block_cache_dir;

    println!("\n{}", "═".repeat(70));
    println!("SORT-MERGE DIFFERENTIAL VALIDATION");
    println!("{}", "═".repeat(70));
    println!("Block cache: {}", block_cache_dir.display());
    println!("Data dir: {}", data_dir.display());
    println!("Block range: {} to {}", start_height, end_height);

    let total_start = Instant::now();

    let step = args[1].as_str();

    match step {
        "step1" | "1" => {
            extract_input_refs(
                chunks_dir,
                &inputs_unsorted,
                start_height,
                end_height,
                progress_interval,
            )?;
        }
        "step2" | "2" => {
            sort_input_refs(&inputs_unsorted, &inputs_sorted)?;
        }
        "step3" | "3" => {
            extract_outputs(
                chunks_dir,
                &outputs_unsorted,
                start_height,
                end_height,
                progress_interval,
            )?;
        }
        "step3b" => {
            sort_outputs(&outputs_unsorted, &outputs_sorted)?;
        }
        "step4" | "4" => {
            merge_join(&inputs_sorted, &outputs_sorted, &joined_unsorted)?;
        }
        "step5" | "5" => {
            sort_joined(&joined_unsorted, &joined_sorted)?;
        }
        "step6" | "6" => {
            let network = Network::Mainnet;
            verify_scripts(
                chunks_dir,
                &joined_sorted,
                start_height,
                end_height,
                progress_interval,
                network,
            )?;
        }
        "all" => {
            // Run all steps
            println!("\nRunning all steps...\n");

            // Step 1: Extract inputs
            extract_input_refs(
                chunks_dir,
                &inputs_unsorted,
                start_height,
                end_height,
                progress_interval,
            )?;

            // Step 2: Sort inputs
            sort_input_refs(&inputs_unsorted, &inputs_sorted)?;

            // Step 3: Extract outputs
            extract_outputs(
                chunks_dir,
                &outputs_unsorted,
                start_height,
                end_height,
                progress_interval,
            )?;

            // Step 3b: Sort outputs
            sort_outputs(&outputs_unsorted, &outputs_sorted)?;

            // Step 4: Merge-join
            merge_join(&inputs_sorted, &outputs_sorted, &joined_unsorted)?;

            // Step 5: Sort joined
            sort_joined(&joined_unsorted, &joined_sorted)?;

            // Step 6: Verify scripts
            let network = Network::Mainnet;
            let (verified, failed, divergences) = verify_scripts(
                chunks_dir,
                &joined_sorted,
                start_height,
                end_height,
                progress_interval,
                network,
            )?;

            // Final summary
            println!("\n{}", "═".repeat(70));
            println!("FINAL RESULTS");
            println!("{}", "═".repeat(70));
            println!("  Scripts verified: {}", verified);
            println!("  Scripts failed: {}", failed);
            println!("  Divergences: {}", divergences.len());

            if !divergences.is_empty() {
                println!("\nFirst 10 divergences:");
                for (i, (height, msg)) in divergences.iter().take(10).enumerate() {
                    println!("  {}. Block {}: {}", i + 1, height, msg);
                }
            }
        }
        "prevout_smoke" => {
            let smoke_start: u32 = get_env("SMOKE_START", "812363").parse()?;
            let smoke_end: u32 = get_env("SMOKE_END", "812400").parse()?;
            smoke_test_prevout_reader(&joined_sorted, smoke_start, smoke_end)?;
        }
        "status" => {
            // Show status of intermediate files
            println!("\nFile Status:");

            let files = [
                ("inputs_unsorted.bin", &inputs_unsorted),
                ("inputs_sorted.bin", &inputs_sorted),
                ("outputs_unsorted.bin", &outputs_unsorted),
                ("outputs_sorted.bin", &outputs_sorted),
                ("joined_unsorted.bin", &joined_unsorted),
                ("joined_sorted.bin", &joined_sorted),
            ];

            for (name, path) in files {
                if path.exists() {
                    let meta = std::fs::metadata(path)?;
                    let size_gb = meta.len() as f64 / 1_073_741_824.0;
                    println!("  ✓ {} ({:.2} GB)", name, size_gb);
                } else {
                    println!("  ✗ {} (not found)", name);
                }
            }
        }
        "clean" => {
            println!("\nCleaning intermediate files...");
            for file in [
                &inputs_unsorted,
                &inputs_sorted,
                &outputs_unsorted,
                &outputs_sorted,
                &joined_unsorted,
                &joined_sorted,
            ] {
                if file.exists() {
                    std::fs::remove_file(file)?;
                    println!("  Removed: {}", file.display());
                }
            }
            println!("  Done!");
        }
        _ => {
            print_usage();
            return Ok(());
        }
    }

    let total_elapsed = total_start.elapsed();
    println!("\n{}", "═".repeat(70));
    println!("Total time: {:.1}m", total_elapsed.as_secs_f64() / 60.0);
    println!("{}", "═".repeat(70));

    Ok(())
}

fn print_usage() {
    println!("Sort-Merge Differential Validation");
    println!();
    println!("Usage: sort_merge_test <step>");
    println!();
    println!("Steps:");
    println!("  step1, 1     Extract input references (~30 min, ~7 GB)");
    println!("  step2, 2     Sort inputs by prevout (~10 min)");
    println!("  step3, 3     Extract outputs (~30 min, ~15 GB)");
    println!("  step3b       Sort outputs by txid (~15 min)");
    println!("  step4, 4     Merge-join inputs with outputs (~5 min, ~10 GB)");
    println!("  step5, 5     Sort joined by spending location (~10 min)");
    println!("  step6, 6     Verify scripts in parallel (~2-3 hours)");
    println!("  prevout_smoke  Validate PrevoutReader seek (SMOKE_START/SMOKE_END)");
    println!("  all          Run all steps");
    println!("  status       Show status of intermediate files");
    println!("  clean        Remove intermediate files");
    println!();
    println!("Environment variables:");
    println!("  BLOCK_CACHE_DIR    Block cache directory (required)");
    println!("  SORT_MERGE_DIR     Data directory for intermediate files");
    println!("  START_HEIGHT       Starting block height (default: 0)");
    println!("  END_HEIGHT         Ending block height (default: 912723)");
    println!("  PROGRESS_INTERVAL  Progress report interval (default: 10000)");
}
