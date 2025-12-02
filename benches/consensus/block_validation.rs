use bllvm_consensus::block::connect_block;
use bllvm_consensus::segwit::Witness;
use bllvm_consensus::{
    tx_inputs, tx_outputs, Block, BlockHeader, OutPoint, Transaction, TransactionInput,
    TransactionOutput, UtxoSet, UTXO,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn create_test_block() -> Block {
    Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: [0; 32],
            merkle_root: [0; 32],
            timestamp: 1234567890,
            bits: 0x1d00ffff,
            nonce: 0,
        },
        transactions: vec![Transaction {
            version: 1,
            inputs: tx_inputs![],
            outputs: tx_outputs![],
            lock_time: 0,
        }]
        .into_boxed_slice(),
    }
}
fn benchmark_connect_block(c: &mut Criterion) {
    let block = create_test_block();
    // Coinbase transaction doesn't need UTXOs, so empty set is fine
    let utxo_set = UtxoSet::new();
    let witnesses: Vec<Witness> = block.transactions.iter().map(|_| Vec::new()).collect();
    c.bench_function("connect_block", |b| {
        b.iter(|| {
            let _result = connect_block(
                black_box(&block),
                black_box(&witnesses),
                black_box(utxo_set.clone()),
                black_box(0),
                black_box(None),
                black_box(bllvm_consensus::types::Network::Mainnet),
            );
            // Coinbase-only block, so validation should succeed
        })
    });
}

fn benchmark_connect_block_multi_tx(c: &mut Criterion) {
    // Create coinbase transaction
    let coinbase = Transaction {
        version: 1,
        inputs: tx_inputs![TransactionInput {
            prevout: OutPoint {
                hash: [0; 32],
                index: 0xffffffff, // Coinbase
            },
            script_sig: vec![0x51; 4],
            sequence: 0xffffffff,
        }],
        outputs: tx_outputs![TransactionOutput {
            value: 50_000_000_000,
            script_pubkey: vec![0x51],
        }],
        lock_time: 0,
    };
    // Add 10 regular transactions with inputs that reference UTXOs
    let mut transactions = vec![coinbase];
    let mut utxo_set = UtxoSet::new();

    for i in 0..10 {
        // Create a UTXO that will be spent by this transaction
        let prev_outpoint = OutPoint {
            hash: [i as u8; 32],
            index: 0,
        };
        let prev_utxo = UTXO {
            value: 10_000_000,
            script_pubkey: vec![0x51; 25],
            height: 0,
        };
        utxo_set.insert(prev_outpoint.clone(), prev_utxo);

        // Create transaction that spends the UTXO
        transactions.push(Transaction {
            version: 1,
            inputs: tx_inputs![TransactionInput {
                prevout: prev_outpoint.clone(),
                script_sig: vec![0x51; 20],
                sequence: 0xffffffff,
            }],
            outputs: tx_outputs![TransactionOutput {
                value: 5_000_000,
                script_pubkey: vec![0x51; 25],
            }],
            lock_time: 0,
        });
    }
    let block = Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: [0; 32],
            merkle_root: [0; 32],
            timestamp: 1234567890,
            bits: 0x1d00ffff,
            nonce: 0,
        },
        transactions: transactions.into_boxed_slice(),
    };
    let witnesses: Vec<Witness> = block.transactions.iter().map(|_| Vec::new()).collect();
    c.bench_function("connect_block_multi_tx", |b| {
        b.iter(|| {
            let _result = connect_block(
                black_box(&block),
                black_box(&witnesses),
                black_box(utxo_set.clone()),
                black_box(0),
                black_box(None),
                black_box(bllvm_consensus::types::Network::Mainnet),
            );
            // Now with valid UTXOs, this should do actual validation work
        })
    });
}
criterion_group!(
    benches,
    benchmark_connect_block,
    benchmark_connect_block_multi_tx
);
criterion_main!(benches);
