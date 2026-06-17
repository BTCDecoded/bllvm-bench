//! Direct search for block 1 in block files using block_file_reader

#[cfg(feature = "differential")]
use anyhow::Result;
#[cfg(feature = "differential")]
use blvm_bench::block_file_reader::{BlockFileReader, Network as BlockFileNetwork};
#[cfg(feature = "differential")]
use sha2::{Digest, Sha256};

#[test]
#[ignore = "local Bitcoin Core data: set BITCOIN_DATA_DIR and run with --ignored"]
#[cfg(feature = "differential")]
fn find_block1_direct() -> Result<()> {
    std::env::remove_var("BLOCK_CACHE_DIR");

    let data_dir =
        std::path::PathBuf::from(std::env::var("BITCOIN_DATA_DIR").expect("BITCOIN_DATA_DIR"));
    let reader = BlockFileReader::new(data_dir, BlockFileNetwork::Mainnet)?;

    let genesis_hash_be =
        hex::decode("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f")?;
    let block1_hash_be =
        hex::decode("00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048")?;

    println!("🔍 Searching for block 1 DIRECTLY in block files (will search until found)...");
    println!("Genesis hash: {}", hex::encode(&genesis_hash_be));
    println!("Block 1 hash: {}", hex::encode(&block1_hash_be));

    let mut iterator = reader.read_blocks_sequential(None, None)?; // No limit - search until found
    let mut block_num = 0;
    let mut genesis_found = false;

    while let Some(block_result) = iterator.next() {
        let block_data = block_result?;

        if block_data.len() >= 80 {
            // Calculate block hash
            let header = &block_data[0..80];
            let first = Sha256::digest(header);
            let second = Sha256::digest(&first);
            let mut block_hash_be = [0u8; 32];
            block_hash_be.copy_from_slice(&second);
            block_hash_be.reverse();

            // Extract prev_hash
            let prev_hash_le = &block_data[4..36];
            let mut prev_hash_be = [0u8; 32];
            prev_hash_be.copy_from_slice(prev_hash_le);
            prev_hash_be.reverse();

            // Check for genesis
            if !genesis_found && block_hash_be == genesis_hash_be.as_slice() {
                println!("   ✅ Found genesis block at block #{}", block_num);
                genesis_found = true;
            }

            // Check if this is block 1
            if block_hash_be == block1_hash_be.as_slice() {
                println!("\n✅✅✅ FOUND BLOCK 1! ✅✅✅");
                println!("   Block number in sequence: {}", block_num);
                println!("   prev_hash (BE): {}", hex::encode(&prev_hash_be));
                println!("   Genesis (BE): {}", hex::encode(&genesis_hash_be));
                if prev_hash_be == genesis_hash_be.as_slice() {
                    println!("   ✅✅✅ prev_hash MATCHES genesis!");
                    println!("\n🎯 SUCCESS: Block 1 found with correct prev_hash!");
                    println!(
                        "   Now we know block 1 exists in the files and has correct prev_hash."
                    );
                    println!("   The issue must be in how chunk_index processes or chains blocks.");
                } else {
                    println!("   ❌ prev_hash does NOT match genesis");
                    println!("   This is the bug - block 1's prev_hash should match genesis");
                }
                return Ok(());
            }

            // Also check by prev_hash
            if prev_hash_be == genesis_hash_be.as_slice()
                && block_hash_be != genesis_hash_be.as_slice()
            {
                println!("\n✅ Found block with genesis as prev_hash!");
                println!("   Block number: {}", block_num);
                println!("   block_hash (BE): {}", hex::encode(&block_hash_be));
                println!("   Expected block 1: {}", hex::encode(&block1_hash_be));
                if block_hash_be == block1_hash_be.as_slice() {
                    println!("   ✅✅✅ THIS IS BLOCK 1!");
                    return Ok(());
                }
            }
        }

        block_num += 1;
        if block_num % 10000 == 0 {
            println!("   Processed {} blocks...", block_num);
        }
    }

    println!("\n❌ Block 1 not found after processing all blocks");
    Ok(())
}
