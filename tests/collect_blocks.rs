//! Collection-only test - fast block collection without validation
//! Validation happens during chunking

#[cfg(feature = "differential")]
use anyhow::Result;
#[cfg(feature = "differential")]
use blvm_bench::block_file_reader::{BlockFileReader, Network as BlockFileNetwork};
#[cfg(feature = "differential")]
use std::path::PathBuf;

/// Collect blocks only (no validation during collection)
/// Validation happens during chunking
#[tokio::test]
#[cfg(feature = "differential")]
async fn collect_blocks_only() -> Result<()> {
    println!("🚀 Starting collection-only mode");
    println!("   Blocks will be validated during chunking");
    
    // Get data directory from environment or auto-detect
    let data_dir = std::env::var("BITCOIN_DATA_DIR")
        .ok()
        .map(PathBuf::from);
    
    let cache_dir = std::env::var("BLOCK_CACHE_DIR")
        .ok()
        .map(PathBuf::from);
    
    // Create block file reader
    let reader = if let Some(dir) = data_dir {
        BlockFileReader::new(dir, BlockFileNetwork::Mainnet)?
    } else {
        BlockFileReader::auto_detect(BlockFileNetwork::Mainnet)?
    };
    
    println!("📂 Block file reader created");
    println!("💾 Cache directory: {:?}", cache_dir);
    println!("");
    println!("   Collection will:");
    println!("   - Read blocks sequentially (fast)");
    println!("   - Write to temp file");
    println!("   - Chunk every 125,000 blocks");
    println!("   - Validate blocks during chunking");
    println!("   - Compress and move chunks to secondary drive");
    println!("");
    
    // Read all blocks sequentially - this triggers collection
    // The iterator will automatically write to temp file and chunk incrementally
    let mut iterator = reader.read_blocks_sequential(None, None)?;
    
    let mut count = 0;
    let start_time = std::time::Instant::now();
    let mut last_report = std::time::Instant::now();
    
    while let Some(block_result) = iterator.next() {
        match block_result {
            Ok(_block_data) => {
                count += 1;
                
                // Progress reporting every 10k blocks
                if count % 10000 == 0 {
                    let elapsed = last_report.elapsed().as_secs_f64();
                    let rate = if elapsed > 0.0 {
                        10000.0 / elapsed
                    } else {
                        0.0
                    };
                    let total_elapsed = start_time.elapsed().as_secs_f64();
                    let avg_rate = if total_elapsed > 0.0 {
                        count as f64 / total_elapsed
                    } else {
                        0.0
                    };
                    
                    println!("   📊 Collected {} blocks | Rate: {:.0} blocks/sec (avg: {:.0})", 
                             count, rate, avg_rate);
                    last_report = std::time::Instant::now();
                }
            }
            Err(e) => {
                eprintln!("   ⚠️  Error reading block {}: {}", count, e);
                return Err(e);
            }
        }
    }
    
    let total_time = start_time.elapsed();
    let avg_rate = if total_time.as_secs_f64() > 0.0 {
        count as f64 / total_time.as_secs_f64()
    } else {
        0.0
    };
    
    println!("");
    println!("✅ Collection complete!");
    println!("   Total blocks: {}", count);
    println!("   Total time: {:.1} minutes", total_time.as_secs_f64() / 60.0);
    println!("   Average rate: {:.0} blocks/sec", avg_rate);
    
    Ok(())
}
