//! CheckBlock Benchmark (Structure Validation Only)
//! Matches Core's DeserializeAndCheckBlockTest benchmark
//!
//! This benchmark:
//! 1. Creates a block with transactions
//! 2. Calls check_block (structure validation only, no scripts)
//! 3. Matches Core's CheckBlock operation (not connect_block)

use blvm_protocol::mining::calculate_merkle_root;
use blvm_protocol::validation::ProtocolValidationContext;
use blvm_protocol::{
    BitcoinProtocolEngine, Block, BlockHeader, OutPoint, ProtocolVersion, Transaction,
    TransactionInput, TransactionOutput, UtxoSet, tx_inputs, tx_outputs,
};
use criterion::{Criterion, black_box, criterion_group, criterion_main};

/// Create a test block for CheckBlock benchmark
/// Matches Core's block413567 structure (simplified for testing)
fn create_test_block_for_check_block() -> Block {
    // Create coinbase transaction
    let coinbase = Transaction {
        version: 1,
        inputs: tx_inputs![TransactionInput {
            prevout: OutPoint {
                hash: [0u8; 32],
                index: 0xffffffff, // Coinbase
            },
            script_sig: vec![blvm_protocol::opcodes::OP_1; 4],
            sequence: 0xffffffff,
        }],
        outputs: tx_outputs![TransactionOutput {
            value: 50_000_000_000, // 50 BTC
            script_pubkey: vec![blvm_protocol::opcodes::OP_1],
        }],
        lock_time: 0,
    };

    // Create some regular transactions (simplified - no real signatures needed for CheckBlock)
    let mut transactions = vec![coinbase];
    for i in 0..100 {
        let tx = Transaction {
            version: 1,
            inputs: tx_inputs![TransactionInput {
                prevout: OutPoint {
                    hash: [i as u8; 32],
                    index: 0,
                },
                script_sig: vec![blvm_protocol::opcodes::OP_1],
                sequence: 0xffffffff,
            }],
            outputs: tx_outputs![TransactionOutput {
                value: 10_000_000,
                script_pubkey: vec![blvm_protocol::opcodes::OP_1],
            }],
            lock_time: 0,
        };
        transactions.push(tx);
    }

    // Calculate merkle root
    let merkle_root = calculate_merkle_root(&transactions).unwrap_or([0u8; 32]);

    Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: [1u8; 32],
            merkle_root,
            timestamp: 1234567890,
            bits: 0x207fffff, // Regtest difficulty
            nonce: 0,
        },
        transactions: transactions.into_boxed_slice(),
    }
}

fn benchmark_check_block(c: &mut Criterion) {
    let block = create_test_block_for_check_block();
    let engine = BitcoinProtocolEngine::new(ProtocolVersion::Regtest).expect("protocol engine");
    let context = ProtocolValidationContext::new(ProtocolVersion::Regtest, 0).expect("context");
    let utxos = UtxoSet::default();

    c.bench_function("check_block", |b| {
        b.iter(|| {
            let result = engine
                .validate_block_with_protocol(black_box(&block), &utxos, 0, &context)
                .expect("validate");
            let _ = black_box(result);
        })
    });
}

criterion_group!(benches, benchmark_check_block);
criterion_main!(benches);
