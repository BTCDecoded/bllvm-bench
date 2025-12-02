//! Transaction ID Benchmark
//! Matches Core's TransactionIdCalculation benchmark exactly

use bllvm_consensus::block::calculate_tx_id;
use bllvm_consensus::{
    tx_inputs, tx_outputs, OutPoint, Transaction, TransactionInput, TransactionOutput,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn create_test_transaction() -> Transaction {
    // Matches Core's transaction structure: 2 inputs, 2 outputs
    Transaction {
        version: 1,
        inputs: tx_inputs![
            TransactionInput {
                prevout: OutPoint {
                    hash: [1; 32],
                    index: 1,
                },
                script_sig: vec![0u8; 65], // 65 bytes (matches Core)
                sequence: 0xffffffff,
            },
            TransactionInput {
                prevout: OutPoint {
                    hash: [2; 32],
                    index: 0,
                },
                script_sig: {
                    let mut sig = vec![0u8; 65];
                    sig.extend_from_slice(&vec![4u8; 33]); // 65 + 33 bytes (matches Core)
                    sig
                },
                sequence: 0xffffffff,
            }
        ],
        outputs: tx_outputs![
            TransactionOutput {
                value: 90_000_000_000,     // 90 BTC (matches Core's 90 * COIN)
                script_pubkey: vec![0x51], // OP_1 (matches Core)
            },
            TransactionOutput {
                value: 10_000_000_000,     // 10 BTC (matches Core's 10 * COIN)
                script_pubkey: vec![0x51], // OP_1 (matches Core)
            }
        ],
        lock_time: 0,
    }
}

fn benchmark_transaction_id_calculation(c: &mut Criterion) {
    let tx = create_test_transaction();

    c.bench_function("transaction_id_calculation", |b| {
        b.iter(|| {
            // Transaction ID is calculated as double SHA256 of serialized transaction (without witness)
            let txid = calculate_tx_id(black_box(&tx));
            black_box(txid);
        })
    });
}

criterion_group!(benches, benchmark_transaction_id_calculation);
criterion_main!(benches);
