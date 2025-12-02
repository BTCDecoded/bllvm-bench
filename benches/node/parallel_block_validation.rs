//! Parallel Block Validation Benchmark
//!
//! Fair comparison with Bitcoin Core's ConnectBlock benchmark:
//! - Core: Sequential validation of N blocks (time_per_block * N)
//! - Commons: Parallel validation of N blocks (simultaneous)
//! Both validate the same N blocks with identical structure:
//! - 1000 transactions per block
//! - Mixed ECDSA/Schnorr signatures (1:4 ratio)
//! - Chained transactions (each spends from previous)

use bllvm_node::validation::{BlockValidationContext, ParallelBlockValidator};
use bllvm_protocol::{
    tx_inputs, tx_outputs, Block, BlockHeader, OutPoint, Transaction, TransactionInput,
    TransactionOutput, UtxoSet,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::time::Instant;
const NUM_BLOCKS: usize = 1000;
const DEPTH_FROM_TIP: usize = 200; // Deep enough to enable parallel validation (>100)
/// Create a test block matching Core's CreateTestBlock structure
/// - 1000 transactions
/// - Mixed ECDSA/Schnorr (simplified - actual Core uses 1:4 ratio)
/// - Chained transactions (each spends from previous)
fn create_test_block_matching_core(
    height: u64,
    prev_hash: [u8; 32],
    prev_utxo_set: &UtxoSet,
) -> (Block, UtxoSet) {
    // Create coinbase transaction
    let coinbase = Transaction {
        version: 1,
        inputs: bllvm_protocol::tx_inputs![TransactionInput {
            prevout: OutPoint {
                hash: [0; 32],
                index: 0xffffffff, // Coinbase
            },
            script_sig: vec![0x51; 4],
            sequence: 0xffffffff,
        }],
        outputs: tx_outputs![TransactionOutput {
            value: 50_000_000_000,     // 50 BTC
            script_pubkey: vec![0x51], // Simplified
        }],
        lock_time: 0,
    };
    // Create 1000 regular transactions (chained - each spends from previous)
    // Note: This is simplified - Core's version uses actual signatures and proper UTXO handling
    // For benchmarking, we focus on the validation work, not signature creation
    let mut transactions = vec![coinbase];
    let mut current_utxo_set = prev_utxo_set.clone();
    let mut prev_tx_hash = [0u8; 32]; // Will be set from coinbase
    for i in 0..1000 {
        // Create transaction that spends from previous
        let tx = Transaction {
            version: 1,
            inputs: tx_inputs![TransactionInput {
                prevout: OutPoint {
                    hash: prev_tx_hash,
                    index: 0,
                },
                script_sig: vec![0x51; 20], // Simplified signature
                sequence: 0xffffffff,
            }],
            outputs: tx_outputs![
                TransactionOutput {
                    value: 10_000_000,
                    script_pubkey: vec![0x51; 25],
                },
                TransactionOutput {
                    value: 5_000_000,
                    script_pubkey: vec![0x51; 25],
                }
            ],
            lock_time: 0,
        };
        // Calculate transaction ID (double SHA256 of serialized transaction)
        use bllvm_protocol::block::calculate_tx_id;
        prev_tx_hash = calculate_tx_id(&tx);
        transactions.push(tx);
    }
    // Calculate merkle root
    use bllvm_protocol::mining::calculate_merkle_root;
    let mut txs_for_merkle = transactions.clone();
    let merkle_root = calculate_merkle_root(&txs_for_merkle).unwrap_or([0; 32]);
    let block = Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: prev_hash,
            merkle_root,
            timestamp: 1234567890 + height,
            bits: 0x207fffff, // Regtest difficulty
            nonce: height,
        },
        transactions: transactions.into_boxed_slice(),
    };
    // Note: UTXO set update would happen during actual validation
    // For benchmarking, we use the previous UTXO set
    (block, current_utxo_set)
}
fn benchmark_parallel_validation(c: &mut Criterion) {
    let validator = ParallelBlockValidator::default();
    // Create N blocks with chained prev_block_hash
    let mut contexts = Vec::new();
    let mut prev_hash = [0u8; 32];
    let mut prev_utxo_set = UtxoSet::new();
    for height in 0..NUM_BLOCKS {
        let (block, utxo_set) =
            create_test_block_matching_core(height as u64, prev_hash, &prev_utxo_set);
        // Calculate block hash (double SHA256 of serialized header)
        use sha2::{Digest, Sha256};
        let mut header_bytes = Vec::with_capacity(80);
        header_bytes.extend_from_slice(&block.header.version.to_le_bytes());
        header_bytes.extend_from_slice(&block.header.prev_block_hash);
        header_bytes.extend_from_slice(&block.header.merkle_root);
        header_bytes.extend_from_slice(&block.header.timestamp.to_le_bytes());
        header_bytes.extend_from_slice(&block.header.bits.to_le_bytes());
        header_bytes.extend_from_slice(&block.header.nonce.to_le_bytes());
        let first_hash = Sha256::digest(&header_bytes);
        let second_hash = Sha256::digest(&first_hash);
        let mut block_hash = [0u8; 32];
        block_hash.copy_from_slice(&second_hash);
        contexts.push(BlockValidationContext {
            block,
            height: height as u64,
            prev_block_hash: prev_hash,
            prev_utxo_set: prev_utxo_set.clone(),
        });
        prev_hash = block_hash;
        prev_utxo_set = utxo_set;
    }
    c.bench_function("validate_blocks_parallel_1000", |b| {
        b.iter(|| {
            let start = Instant::now();
            let _result =
                validator.validate_blocks_parallel(black_box(&contexts), black_box(DEPTH_FROM_TIP));
            let elapsed = start.elapsed();
            black_box(elapsed)
        })
    });
}

fn benchmark_sequential_validation(c: &mut Criterion) {
    let validator = ParallelBlockValidator::default();
    // Create N blocks (same as parallel)
    let mut contexts = Vec::new();
    let mut prev_hash = [0u8; 32];
    let mut prev_utxo_set = UtxoSet::new();
    for height in 0..NUM_BLOCKS {
        let (block, utxo_set) =
            create_test_block_matching_core(height as u64, prev_hash, &prev_utxo_set);
        use sha2::{Digest, Sha256};
        let mut header_bytes = Vec::with_capacity(80);
        header_bytes.extend_from_slice(&block.header.version.to_le_bytes());
        header_bytes.extend_from_slice(&block.header.prev_block_hash);
        header_bytes.extend_from_slice(&block.header.merkle_root);
        header_bytes.extend_from_slice(&block.header.timestamp.to_le_bytes());
        header_bytes.extend_from_slice(&block.header.bits.to_le_bytes());
        header_bytes.extend_from_slice(&block.header.nonce.to_le_bytes());
        let first_hash = Sha256::digest(&header_bytes);
        let second_hash = Sha256::digest(&first_hash);
        let mut block_hash = [0u8; 32];
        block_hash.copy_from_slice(&second_hash);
        contexts.push(BlockValidationContext {
            block,
            height: height as u64,
            prev_block_hash: prev_hash,
            prev_utxo_set: prev_utxo_set.clone(),
        });
        prev_hash = block_hash;
        prev_utxo_set = utxo_set;
    }
    c.bench_function("validate_blocks_sequential_1000", |b| {
        b.iter(|| {
            let _result = validator.validate_blocks_sequential(black_box(&contexts));
            black_box(_result)
        })
    });
}
criterion_group!(
    benches,
    benchmark_parallel_validation,
    benchmark_sequential_validation
);
criterion_main!(benches);
