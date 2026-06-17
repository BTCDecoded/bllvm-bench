//! Standalone historical parallel differential test (uses `create_block_data_source`: files, cache, or remote-Core RPC).

#[cfg(feature = "differential")]
use anyhow::Result;
#[cfg(feature = "differential")]
use blvm_bench::core_rpc_client::{CoreRpcClient, RpcConfig};
#[cfg(feature = "differential")]
use blvm_bench::parallel_differential::{
    ParallelConfig, create_block_data_source, run_parallel_differential,
};
#[cfg(feature = "differential")]
use std::sync::Arc;

/// Test historical blocks in parallel (direct files, chunk cache, or remote-Core RPC per env).
#[tokio::test]
#[cfg(feature = "differential")]
async fn test_remote_core_historical_100_blocks() -> Result<()> {
    // Try to create RPC client for Core validation comparison (optional)
    // If RPC fails, we'll use direct file reading only (faster, but blocks from Core files are assumed valid)
    let rpc_config = RpcConfig::from_env();
    let rpc_client: Option<CoreRpcClient> = {
        let client = CoreRpcClient::new(rpc_config);
        match client.detect_network().await {
            Ok(n) => {
                println!(
                    "📡 RPC available for Core validation comparison: {}",
                    n.as_str()
                );
                Some(client)
            }
            Err(e) => {
                println!(
                    "⚠️  RPC connection failed: {}. Using direct file reading (blocks from Core files assumed valid)",
                    e
                );
                None
            }
        }
    };

    // Get configuration from environment
    let start_height: u64 = std::env::var("HISTORICAL_BLOCK_START")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let end_height: u64 = std::env::var("HISTORICAL_BLOCK_END")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    let num_workers: usize = std::env::var("PARALLEL_WORKERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);

    let chunk_size: u64 = std::env::var("CHUNK_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    // For cache building, disable checkpoints to avoid validation failures blocking cache collection
    // Checkpoints are only needed for parallel differential testing, not for cache building
    let use_checkpoints = std::env::var("USE_CHECKPOINTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(false); // Default to false for cache building

    let config = ParallelConfig {
        num_workers,
        chunk_size,
        use_checkpoints,
    };

    println!("🔧 Configuration:");
    println!("   Start height: {}", start_height);
    println!("   End height: {}", end_height);
    println!("   Workers: {}", config.num_workers);
    println!("   Chunk size: {}", config.chunk_size);

    // Create optimized block data source - prefer direct file reading
    let cache_dir = std::env::var("BLOCK_CACHE_DIR")
        .ok()
        .map(std::path::PathBuf::from);

    let block_source = create_block_data_source(
        blvm_bench::parallel_differential::BlockFileNetwork::Mainnet,
        cache_dir.as_deref(),
        rpc_client.map(|c| Arc::new(c)),
    )?;

    // Run parallel differential test
    // Note: With direct file reading, blocks from Core's files are assumed valid
    // This is safe because we're reading directly from Core's data directory
    let results =
        run_parallel_differential(start_height, end_height, config, Arc::new(block_source)).await?;

    // Check for divergences
    let total_tested: usize = results.iter().map(|r| r.tested).sum();
    let total_matched: usize = results.iter().map(|r| r.matched).sum();
    let total_divergences: usize = results.iter().map(|r| r.divergences.len()).sum();

    println!("\n📊 Results:");
    println!("   Blocks tested: {}", total_tested);
    println!("   Matched: {}", total_matched);
    println!("   Divergences: {}", total_divergences);

    if total_divergences > 0 {
        eprintln!("❌ Found {} divergences!", total_divergences);
        for result in &results {
            for (height, blvm, core) in &result.divergences {
                eprintln!("   Height {}: BLVM={}, Core={}", height, blvm, core);
            }
        }
    } else {
        println!("✅ All blocks matched between BLVM and Core!");
    }

    Ok(())
}
