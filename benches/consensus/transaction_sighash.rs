//! Transaction Sighash Benchmark
//! Matches Core's TransactionSighashCalculation benchmark exactly

use blvm_protocol::transaction_hash::{SighashType, calculate_transaction_sighash};
use blvm_protocol::{
    OutPoint, Transaction, TransactionInput, TransactionOutput, tx_inputs, tx_outputs,
};
use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn create_test_transaction() -> (Transaction, Vec<TransactionOutput>) {
    // Matches Core's transaction structure: 1 input, 1 output
    let tx = Transaction {
        version: 1,
        inputs: tx_inputs![TransactionInput {
            prevout: OutPoint {
                hash: [1; 32],
                index: 0,
            },
            script_sig: vec![blvm_protocol::opcodes::OP_1],
            sequence: 0xffffffff,
        }],
        outputs: tx_outputs![TransactionOutput {
            value: 10_000_000_000, // 10 BTC (matches Core's 10 * COIN)
            script_pubkey: vec![
                blvm_protocol::opcodes::OP_1,
                blvm_protocol::opcodes::OP_EQUAL
            ],
        }],
        lock_time: 0,
    };

    // Create prevout (matches Core's coin structure)
    let prevouts = vec![TransactionOutput {
        value: 11_000_000_000, // 11 BTC (matches Core's dummy input value)
        script_pubkey: vec![
            blvm_protocol::opcodes::OP_1,
            blvm_protocol::opcodes::OP_EQUAL,
        ],
    }];

    (tx, prevouts)
}

fn benchmark_transaction_sighash_calculation(c: &mut Criterion) {
    let (tx, prevouts) = create_test_transaction();

    c.bench_function("transaction_sighash_calculation", |b| {
        b.iter(|| {
            // Calculate sighash (SIGHASH_ALL is most common, matches Core)
            // Core uses SigVersion::BASE (legacy) to match Commons
            black_box(
                calculate_transaction_sighash(
                    black_box(&tx),
                    0, // input index
                    black_box(&prevouts),
                    SighashType::ALL,
                )
                .unwrap(),
            )
        })
    });
}

criterion_group!(benches, benchmark_transaction_sighash_calculation);
criterion_main!(benches);
