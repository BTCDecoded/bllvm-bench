//! Transaction Serialization Benchmark
//! Matches Core's TransactionSerialization benchmark

use blvm_protocol::serialization::transaction::serialize_transaction;
use blvm_protocol::{
    OutPoint, Transaction, TransactionInput, TransactionOutput, tx_inputs, tx_outputs,
};
use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn create_test_transaction() -> Transaction {
    // Create a transaction similar to Core's benchmark
    Transaction {
        version: 1,
        inputs: tx_inputs![TransactionInput {
            prevout: OutPoint {
                hash: [1; 32],
                index: 0,
            },
            script_sig: vec![blvm_protocol::opcodes::OP_1; 20],
            sequence: 0xffffffff,
        }],
        outputs: tx_outputs![TransactionOutput {
            value: 10_000_000_000,
            script_pubkey: vec![blvm_protocol::opcodes::OP_1; 25],
        }],
        lock_time: 0,
    }
}

fn benchmark_transaction_serialization(c: &mut Criterion) {
    let tx = create_test_transaction();

    c.bench_function("transaction_serialization", |b| {
        b.iter(|| black_box(serialize_transaction(black_box(&tx))))
    });
}

criterion_group!(benches, benchmark_transaction_serialization);
criterion_main!(benches);
