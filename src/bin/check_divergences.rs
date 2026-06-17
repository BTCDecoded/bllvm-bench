//! Check if BLVM failures are divergences from Bitcoin Core
//! BATCHED VERSION: Sends multiple txs per RPC call for speed

use anyhow::Result;
use blvm_bench::remote_core_rpc::RemoteCoreRpcClient;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::time::Instant;

const BATCH_SIZE: usize = 25; // Txs per RPC call

#[derive(Clone)]
struct FailureEntry {
    block_height: u64,
    tx_idx: usize,
    tx_hex: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let limit: Option<usize> = args
        .iter()
        .position(|a| a == "--limit")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());

    let sm = blvm_bench::block_cache_env::sort_merge_data_dir()?;
    let hex_file = sm.join("failures_with_hex.log");
    let original_file = sm.join("failures.log");
    let failures_file = if hex_file.exists() {
        hex_file
    } else {
        original_file
    };

    println!("🔍 Divergence Checker (BATCHED TX HEX MODE)");
    println!("  Batch size: {BATCH_SIZE} txs/call");
    if let Some(l) = limit {
        println!("  Limit: {l} failures");
    }
    println!("  File: {}", failures_file.display());
    println!();

    // Initialize RPC
    println!("Initializing...");
    let rpc_client = RemoteCoreRpcClient::new();
    if let Ok(r) = rpc_client
        .call("getblockcount", serde_json::json!([]))
        .await
    {
        println!(
            "  ✅ Core at block {}",
            r.get("result").and_then(|v| v.as_u64()).unwrap_or(0)
        );
    }

    // Read and parse failures
    let file = File::open(&failures_file)?;
    let reader = BufReader::new(file);

    let mut failures: Vec<FailureEntry> = Vec::new();
    let mut total_failures = 0usize;
    let mut missing_hex = 0usize;

    for line in reader.lines().filter_map(|l| l.ok()) {
        if !line.contains("Script returned false") {
            continue;
        }
        total_failures += 1;

        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 4 {
            missing_hex += 1;
            continue;
        }

        let block_height: u64 = match parts[0].trim().parse() {
            Ok(h) if h > 0 => h,
            _ => continue,
        };

        let msg = parts[2].trim();
        let tx_idx: usize = match msg.find("tx ").and_then(|s| {
            msg[s + 3..]
                .find(',')
                .and_then(|e| msg[s + 3..s + 3 + e].trim().parse().ok())
        }) {
            Some(t) => t,
            None => continue,
        };

        let tx_hex = parts[3].trim().to_string();
        if tx_hex.len() < 20 {
            missing_hex += 1;
            continue;
        }

        failures.push(FailureEntry {
            block_height,
            tx_idx,
            tx_hex,
        });
    }

    println!("  Total script failures: {total_failures}");
    println!("  With TX hex: {}", failures.len());
    if missing_hex > 0 {
        println!("  ⚠️ Missing hex: {missing_hex}");
    }

    if failures.is_empty() {
        println!("\n❌ No failures with TX hex found!");
        return Ok(());
    }

    let process_limit = limit.unwrap_or(failures.len()).min(failures.len());
    let num_batches = process_limit.div_ceil(BATCH_SIZE);
    println!(
        "\nProcessing {process_limit} failures in ~{num_batches} batches...\n"
    );

    // Stats
    let mut divergences = 0u64;
    let mut core_rejects = 0usize;
    let mut skipped_missing = 0usize;
    let mut rpc_errors = 0usize;
    let mut processed = 0usize;
    let mut rpc_calls = 0usize;
    let mut checked_keys: HashSet<String> = HashSet::new();

    let start = Instant::now();

    println!("================================================================================");

    // Process in batches
    let mut batch: Vec<&FailureEntry> = Vec::with_capacity(BATCH_SIZE);

    for failure in failures.iter().take(process_limit) {
        // Deduplicate by tx hex prefix
        let key = if failure.tx_hex.len() >= 64 {
            &failure.tx_hex[..64]
        } else {
            &failure.tx_hex
        };
        if checked_keys.contains(key) {
            continue;
        }
        checked_keys.insert(key.to_string());

        batch.push(failure);

        // Process batch when full
        if batch.len() >= BATCH_SIZE {
            rpc_calls += 1;
            let hex_refs: Vec<&str> = batch.iter().map(|f| f.tx_hex.as_str()).collect();

            match tokio::time::timeout(
                std::time::Duration::from_secs(120),
                rpc_client.test_mempool_accept_batch(&hex_refs),
            )
            .await
            {
                Ok(Ok(results)) => {
                    if let Some(arr) = results.as_array() {
                        for (i, r) in arr.iter().enumerate() {
                            if i >= batch.len() {
                                break;
                            }
                            processed += 1;

                            let allowed =
                                r.get("allowed").and_then(|v| v.as_bool()).unwrap_or(false);
                            let reason = r
                                .get("reject-reason")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");

                            if !allowed && reason.contains("missing-input") {
                                skipped_missing += 1;
                            } else if allowed {
                                divergences += 1;
                                let f = &batch[i];
                                println!(
                                    "\n❌ DIVERGENCE #{}: Block {}, tx {}",
                                    divergences, f.block_height, f.tx_idx
                                );
                            } else {
                                core_rejects += 1;
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    // Batch failed, try individually
                    for f in &batch {
                        processed += 1;
                        if let Ok(Ok(r)) = tokio::time::timeout(
                            std::time::Duration::from_secs(30),
                            rpc_client.test_mempool_accept(&f.tx_hex),
                        )
                        .await
                        {
                            let allowed = r
                                .as_array()
                                .and_then(|a| a.first())
                                .and_then(|x| x.get("allowed"))
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let reason = r
                                .as_array()
                                .and_then(|a| a.first())
                                .and_then(|x| x.get("reject-reason"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");

                            if !allowed && reason.contains("missing-input") {
                                skipped_missing += 1;
                            } else if allowed {
                                divergences += 1;
                                println!(
                                    "\n❌ DIVERGENCE #{}: Block {}, tx {}",
                                    divergences, f.block_height, f.tx_idx
                                );
                            } else {
                                core_rejects += 1;
                            }
                        } else {
                            rpc_errors += 1;
                        }
                    }
                    if rpc_errors < 5 {
                        eprintln!("  Batch RPC error: {e}");
                    }
                }
                Err(_) => {
                    rpc_errors += batch.len();
                    if rpc_errors < 5 {
                        eprintln!("  Batch RPC timeout");
                    }
                }
            }

            batch.clear();

            // Progress report
            if rpc_calls % 10 == 0 || rpc_calls == 1 {
                let elapsed = start.elapsed().as_secs_f64();
                let rate = processed as f64 / elapsed.max(0.1);
                let remaining = (process_limit - processed) as f64 / rate.max(0.1);
                println!(
                    "[{}/{} txs, {} batches] ({:.1} tx/s, ETA {:.0}m) - {} div, {} ok, {} miss, {} err",
                    processed,
                    process_limit,
                    rpc_calls,
                    rate,
                    remaining / 60.0,
                    divergences,
                    core_rejects,
                    skipped_missing,
                    rpc_errors
                );
            }
        }
    }

    // Process remaining batch
    if !batch.is_empty() {
        rpc_calls += 1;
        let hex_refs: Vec<&str> = batch.iter().map(|f| f.tx_hex.as_str()).collect();

        if let Ok(Ok(results)) = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            rpc_client.test_mempool_accept_batch(&hex_refs),
        )
        .await
        {
            if let Some(arr) = results.as_array() {
                for (i, r) in arr.iter().enumerate() {
                    if i >= batch.len() {
                        break;
                    }
                    processed += 1;

                    let allowed = r.get("allowed").and_then(|v| v.as_bool()).unwrap_or(false);
                    let reason = r
                        .get("reject-reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if !allowed && reason.contains("missing-input") {
                        skipped_missing += 1;
                    } else if allowed {
                        divergences += 1;
                        let f = &batch[i];
                        println!(
                            "\n❌ DIVERGENCE #{}: Block {}, tx {}",
                            divergences, f.block_height, f.tx_idx
                        );
                    } else {
                        core_rejects += 1;
                    }
                }
            }
        }
    }

    let elapsed = start.elapsed();

    println!("================================================================================\n");
    println!(
        "RESULTS ({:.1}s = {:.1}m)",
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() / 60.0
    );
    println!("  Txs checked: {processed}");
    println!("  Unique txs: {}", checked_keys.len());
    println!("  RPC calls: {rpc_calls} (batched)");
    println!("  Core also rejects: {core_rejects} ✓");
    println!("  Missing inputs: {skipped_missing}");
    println!("  RPC errors: {rpc_errors}");

    if divergences > 0 {
        println!("\n  ❌ DIVERGENCES: {divergences}");
    } else {
        println!("\n  ✅ NO DIVERGENCES FOUND");
    }

    Ok(())
}
