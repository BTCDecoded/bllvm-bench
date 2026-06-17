//! Collection-only mode - fast block collection without validation
//! Validation happens during chunking or at intervals

use crate::block_file_reader::{BlockFileReader, Network as BlockFileNetwork};
use anyhow::Result;
use std::path::PathBuf;

/// Collect blocks without validation (fast mode)
/// Blocks are validated during chunking or at intervals
pub fn collect_blocks_only(data_dir: Option<PathBuf>, _cache_dir: Option<PathBuf>) -> Result<()> {
    println!("🚀 Starting collection-only mode (fast, no validation during collection)");
    println!("   Validation will occur during chunking");

    // Create block file reader
    let reader = if let Some(dir) = data_dir {
        BlockFileReader::new(dir, BlockFileNetwork::Mainnet)?
    } else {
        BlockFileReader::auto_detect(BlockFileNetwork::Mainnet)?
    };

    println!("📂 Block file reader created");

    // Read all blocks sequentially - this triggers collection
    // The iterator will automatically write to temp file and chunk incrementally
    let mut iterator = reader.read_blocks_sequential(None, None)?;

    let mut count = 0;
    while let Some(block_result) = iterator.next() {
        match block_result {
            Ok(_block_data) => {
                count += 1;
                if count % 10000 == 0 {
                    println!("   📊 Collected {} blocks...", count);
                }
            }
            Err(e) => {
                eprintln!("   ⚠️  Error reading block: {}", e);
                return Err(e);
            }
        }
    }

    println!("✅ Collection complete: {} blocks collected", count);
    Ok(())
}
