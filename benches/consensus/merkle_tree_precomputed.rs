use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use sha2::{Digest, Sha256};

type Hash = [u8; 32];
/// Create pre-computed random hashes (matching Core's MerkleRoot benchmark approach)
fn create_precomputed_hashes(count: usize) -> Vec<Hash> {
    (0..count)
        .map(|i| {
            let mut hasher = Sha256::new();
            hasher.update(&i.to_le_bytes());
            hasher.update(&(i * 7).to_le_bytes()); // Add some variation
            let hash = hasher.finalize();
            let mut result = [0u8; 32];
            result.copy_from_slice(&hash);
            result
        })
        .collect()
}

/// Double SHA256 hash (Bitcoin's hash function)
fn sha256_hash(data: &[u8]) -> Hash {
    let hash1 = Sha256::digest(data);
    let hash2 = Sha256::digest(&hash1);
    let mut result = [0u8; 32];
    result.copy_from_slice(&hash2);
    result
}

/// Calculate merkle root from pre-computed hashes (matching Core's approach)
/// This is the tree-building logic extracted from calculate_merkle_root
fn calculate_merkle_root_from_hashes(hashes: &[Hash]) -> std::result::Result<Hash, String> {
    if hashes.is_empty() {
        return Err("Cannot calculate merkle root for empty hash list".into());
    }
    let mut working_hashes = hashes.to_vec();
    // Build Merkle tree bottom-up
    while working_hashes.len() > 1 {
        let mut next_level = Vec::with_capacity(working_hashes.len() / 2 + 1);
        // Handle odd number of hashes (duplicate last one)
        if working_hashes.len() & 1 != 0 {
            let last_hash = *working_hashes.last().unwrap();
            working_hashes.push(last_hash);
        }
        // Process pairs
        for chunk in working_hashes.chunks(2) {
            if chunk.len() == 2 {
                let mut combined = Vec::with_capacity(64);
                combined.extend_from_slice(&chunk[0]);
                combined.extend_from_slice(&chunk[1]);
                next_level.push(sha256_hash(&combined));
            } else if chunk.len() == 1 {
                // Odd case: duplicate the hash
                let mut combined = Vec::with_capacity(64);
                combined.extend_from_slice(&chunk[0]);
                combined.extend_from_slice(&chunk[0]);
                next_level.push(sha256_hash(&combined));
            }
        }
        working_hashes = next_level;
    }
    if working_hashes.len() != 1 {
        return Err(format!(
            "Merkle tree calculation must result in exactly 1 hash (root), got {}",
            working_hashes.len()
        ));
    }
    Ok(working_hashes[0])
}

fn benchmark_merkle_root_precomputed(c: &mut Criterion) {
    let mut group = c.benchmark_group("merkle_root_precomputed");

    // Match Core's leaf counts: 1, 10, 100, 1000, 2000, 9001
    let leaf_counts = vec![1, 10, 100, 1000, 2000, 9001];
    for leaf_count in leaf_counts {
        let hashes = create_precomputed_hashes(leaf_count);

        group.bench_with_input(
            BenchmarkId::new("merkle_root_precomputed", format!("{}leaves", leaf_count)),
            &hashes,
            |b, hashes| {
                b.iter(|| {
                    // Use the function that takes pre-computed hashes directly
                    black_box(calculate_merkle_root_from_hashes(black_box(hashes)).unwrap())
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, benchmark_merkle_root_precomputed,);
criterion_main!(benches);
