//! Block Assembly Benchmark
//! Measures block creation from mempool transactions (create_new_block)

use bllvm_consensus::mining::create_new_block;
use bllvm_consensus::{
    tx_inputs, tx_outputs, BlockHeader, OutPoint, Transaction, TransactionInput, TransactionOutput,
    UtxoSet,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn create_test_transaction(i: usize) -> Transaction {
    Transaction {
        version: 1,
        inputs: tx_inputs![TransactionInput {
            prevout: OutPoint {
                hash: {
                    let mut h = [0u8; 32];
                    h[0] = (i % 256) as u8;
                    h
                },
                index: 0,
            },
            script_sig: vec![0x51], // OP_1
            sequence: 0xffffffff,
        }],
        outputs: tx_outputs![TransactionOutput {
            value: 1000000000,
            script_pubkey: vec![0x51], // OP_1
        }],
        lock_time: 0,
    }
}

fn benchmark_assemble_block(c: &mut Criterion) {
    // Create a mempool with transactions (matches Core's AssembleBlock exactly)
    // Core: NUM_BLOCKS=200, COINBASE_MATURITY=100, so txs = 200 - 100 + 1 = 101
    let mut mempool_txs = Vec::new();
    let mut utxo_set = UtxoSet::new();

    // Create 101 transactions in mempool (matches Core exactly)
    for i in 0..101 {
        let tx = create_test_transaction(i);
        mempool_txs.push(tx);
    }

    // Create previous block header
    let prev_header = BlockHeader {
        version: 1,
        prev_block_hash: [0; 32],
        merkle_root: [0; 32],
        timestamp: 1234567890,
        bits: 0x1d00ffff,
        nonce: 0,
    };

    let prev_headers = vec![prev_header.clone()];
    let coinbase_script = vec![0x51];
    let coinbase_address = vec![0x51];

    c.bench_function("assemble_block", |b| {
        b.iter(|| {
            black_box(create_new_block(
                black_box(&utxo_set),
                black_box(&mempool_txs),
                black_box(1),
                black_box(&prev_header),
                black_box(&prev_headers),
                black_box(&coinbase_script),
                black_box(&coinbase_address),
            ))
        })
    });
}

criterion_group!(benches, benchmark_assemble_block);
criterion_main!(benches);
