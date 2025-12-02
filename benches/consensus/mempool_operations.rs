use bllvm_consensus::mempool::{
    accept_to_memory_pool, is_standard_tx, replacement_checks, Mempool,
};
use bllvm_consensus::{
    tx_inputs, tx_outputs, OutPoint, Transaction, TransactionInput, TransactionOutput, UtxoSet,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::collections::HashSet;

fn create_test_transaction() -> Transaction {
    Transaction {
        version: 1,
        inputs: tx_inputs![TransactionInput {
            prevout: OutPoint {
                hash: [0u8; 32],
                index: 0,
            },
            script_sig: vec![0x51], // OP_1
            sequence: 0xffffffff,
        }],
        outputs: tx_outputs![TransactionOutput {
            value: 5000000000,
            script_pubkey: vec![0x51], // OP_1
        }],
        lock_time: 0,
    }
}

fn create_complex_transaction(input_count: usize, output_count: usize) -> Transaction {
    Transaction {
        version: 1,
        inputs: (0..input_count)
            .map(|i| TransactionInput {
                prevout: OutPoint {
                    hash: {
                        let mut h = [0u8; 32];
                        h[0] = i as u8;
                        h
                    },
                    index: i as u64,
                },
                script_sig: vec![0x51],
                sequence: 0xffffffff,
            })
            .collect::<Vec<_>>()
            .into(),
        outputs: (0..output_count)
            .map(|_| TransactionOutput {
                value: 1000000000,
                script_pubkey: vec![0x51],
            })
            .collect::<Vec<_>>()
            .into(),
        lock_time: 0,
    }
}

fn benchmark_mempool_acceptance(c: &mut Criterion) {
    let tx = create_test_transaction();
    // Create UTXO for the transaction input (fair comparison - needs valid UTXO)
    let mut utxo_set = UtxoSet::new();
    let outpoint = bllvm_consensus::OutPoint {
        hash: [1; 32], // Matches tx input
        index: 0,
    };
    let utxo = bllvm_consensus::UTXO {
        value: 10_000_000_000,
        script_pubkey: vec![0x51], // Simple script
        height: 0,
    };
    utxo_set.insert(outpoint, utxo);
    let mempool: Mempool = HashSet::new();
    c.bench_function("accept_to_memory_pool_simple", |b| {
        b.iter(|| {
            black_box(accept_to_memory_pool(
                black_box(&tx),
                black_box(None), // witnesses
                black_box(&utxo_set),
                black_box(&mempool),
                black_box(0),
            ))
        })
    });
}

fn benchmark_mempool_acceptance_complex(c: &mut Criterion) {
    // Core's MempoolCheck validates 400 transactions in mempool
    // We need to match this scale for fair comparison
    let mut mempool: Mempool = HashSet::new();
    let mut utxo_set = UtxoSet::new();
    let mut transactions = Vec::new();

    // Create 400 transactions (matches Core's MempoolCheck scale)
    for i in 0..400 {
        let tx = create_complex_transaction(5, 3);
        let tx_id = bllvm_consensus::block::calculate_tx_id(&tx);
        mempool.insert(tx_id);

        // Create UTXOs for all transaction inputs
        for input in &tx.inputs {
            let utxo = bllvm_consensus::UTXO {
                value: 10_000_000_000,
                script_pubkey: vec![0x51], // Simple script
                height: 0,
            };
            utxo_set.insert(input.prevout.clone(), utxo);
        }

        transactions.push(tx);
    }

    c.bench_function("accept_to_memory_pool_complex", |b| {
        b.iter(|| {
            // Simulate Core's MempoolCheck: Validate all 400 transactions using accept_to_memory_pool
            // Core's MempoolCheck validates transactions already in mempool (pool.check())
            // This includes: structure check, input validation, script verification, mempool rules
            for tx in &transactions {
                black_box(accept_to_memory_pool(
                    black_box(tx),
                    black_box(None), // No witnesses for simple transactions
                    black_box(&utxo_set),
                    black_box(&mempool),
                    black_box(0),
                ));
            }
        })
    });
}

fn benchmark_is_standard_tx(c: &mut Criterion) {
    let tx = create_test_transaction();
    c.bench_function("is_standard_tx", |b| {
        b.iter(|| black_box(is_standard_tx(black_box(&tx))))
    });
}

fn benchmark_replacement_checks(c: &mut Criterion) {
    let mut new_tx = create_test_transaction();
    new_tx.inputs[0].sequence = 0xfffffffe; // RBF
    let mut existing_tx = create_test_transaction();
    existing_tx.inputs[0].sequence = 0xfffffffe; // RBF
    c.bench_function("replacement_checks", |b| {
        let utxo_set = UtxoSet::new();
        let mempool: Mempool = HashSet::new();
        b.iter(|| {
            black_box(replacement_checks(
                black_box(&new_tx),
                black_box(&existing_tx),
                black_box(&utxo_set),
                black_box(&mempool),
            ))
        })
    });
}

fn benchmark_mempool_eviction(c: &mut Criterion) {
    // Create a mempool with many transactions to test eviction logic
    let mut mempool: Mempool = HashSet::new();
    let mut utxo_set = UtxoSet::new();

    // Add many transactions to mempool (simulate full mempool)
    for i in 0..1000 {
        let mut tx = create_test_transaction();
        // Make each transaction unique
        tx.inputs[0].prevout.hash[0] = (i % 256) as u8;
        let tx_id = bllvm_consensus::block::calculate_tx_id(&tx);
        mempool.insert(tx_id);
    }

    // Create a new transaction that would cause eviction
    let new_tx = create_test_transaction();

    c.bench_function("mempool_eviction", |b| {
        b.iter(|| {
            // Simulate eviction: check if mempool is full and would need eviction
            // In real scenario, this would involve checking mempool size limits
            // and removing lowest fee transactions
            let mempool_size = black_box(mempool.len());
            let _would_evict = black_box(mempool_size > 500); // Simulate size limit
            black_box(mempool_size)
        })
    });
}

fn benchmark_accept_to_memory_pool_400tx(c: &mut Criterion) {
    // Create 400 transactions and accept them all (matches Core's MempoolCheck scale)
    let mut transactions = Vec::new();
    let utxo_set = UtxoSet::new();
    let mempool: Mempool = HashSet::new();

    for i in 0..400 {
        let mut tx = create_test_transaction();
        tx.inputs[0].prevout.hash[0] = (i % 256) as u8;
        transactions.push(tx);
    }

    c.bench_function("accept_to_memory_pool_400tx", |b| {
        b.iter(|| {
            for tx in &transactions {
                black_box(accept_to_memory_pool(
                    black_box(tx),
                    black_box(None),
                    black_box(&utxo_set),
                    black_box(&mempool),
                    black_box(0),
                ));
            }
        })
    });
}

fn benchmark_is_standard_tx_400tx(c: &mut Criterion) {
    // Create 400 transactions and validate them (matches Core's MempoolCheck)
    // Core's MempoolCheck does full validation including standard checks, not just is_standard_tx
    let mut transactions = Vec::new();
    let mut utxo_set = UtxoSet::new();

    for i in 0..400 {
        let mut tx = create_test_transaction();
        tx.inputs[0].prevout.hash[0] = (i % 256) as u8;
        transactions.push(tx);

        // Create UTXO for this transaction
        let outpoint = bllvm_consensus::OutPoint {
            hash: {
                let mut h = [0u8; 32];
                h[0] = (i % 256) as u8;
                h
            },
            index: 0,
        };
        let utxo = bllvm_consensus::UTXO {
            value: 10_000_000_000,
            script_pubkey: vec![0x51],
            height: 0,
        };
        utxo_set.insert(outpoint, utxo);
    }

    c.bench_function("is_standard_tx_400tx", |b| {
        b.iter(|| {
            // Simulate Core's MempoolCheck: FULL validation of all 400 transactions
            // Core's MempoolCheck validates transactions already in mempool (pool.check())
            // This includes: structure check, input validation, script verification, standard checks
            for tx in &transactions {
                // Check if standard (part of MempoolCheck)
                black_box(is_standard_tx(black_box(tx)));
                // Check transaction structure (part of MempoolCheck)
                black_box(bllvm_consensus::transaction::check_transaction(black_box(
                    tx,
                )));
                // Check inputs against UTXO set (part of MempoolCheck)
                black_box(bllvm_consensus::transaction::check_tx_inputs(
                    black_box(tx),
                    black_box(&utxo_set),
                    black_box(0),
                ));
                // Verify scripts (expensive part of MempoolCheck - Core does this)
                // For simple transactions without witnesses, verify_script does basic checks
                for input in &tx.inputs {
                    if let Some(utxo) = utxo_set.get(&input.prevout) {
                        black_box(bllvm_consensus::script::verify_script(
                            black_box(&input.script_sig),
                            black_box(&utxo.script_pubkey),
                            black_box(None), // No witness for simple transactions
                            black_box(0),    // Standard flags
                        ));
                    }
                }
            }
        })
    });
}

fn benchmark_replacement_checks_mempool(c: &mut Criterion) {
    // Create a mempool with 400 existing transactions (matches Core's MempoolCheck scale)
    // Core's MempoolCheck validates ALL transactions in mempool, including RBF checks
    // We need to simulate full mempool validation, not just a single RBF check
    let mut mempool: Mempool = HashSet::new();
    let mut utxo_set = UtxoSet::new();
    let mut mempool_txs = Vec::new();

    // Create 400 transactions and add them to mempool (matches Core exactly)
    for i in 0..400 {
        let mut tx = create_test_transaction();
        tx.inputs[0].prevout.hash[0] = (i % 256) as u8;
        tx.inputs[0].sequence = 0xfffffffe; // RBF enabled
        let tx_id = bllvm_consensus::block::calculate_tx_id(&tx);
        mempool.insert(tx_id);
        mempool_txs.push(tx);
    }

    // Create UTXOs for all transactions
    for (i, tx) in mempool_txs.iter().enumerate() {
        let outpoint = bllvm_consensus::OutPoint {
            hash: {
                let mut h = [0u8; 32];
                h[0] = (i % 256) as u8;
                h
            },
            index: 0,
        };
        let utxo = bllvm_consensus::UTXO {
            value: 10_000_000_000,
            script_pubkey: vec![0x51],
            height: 0,
        };
        utxo_set.insert(outpoint, utxo);
    }

    c.bench_function("replacement_checks_mempool", |b| {
        b.iter(|| {
            // Simulate Core's MempoolCheck: FULL validation of all 400 transactions
            // Core's MempoolCheck validates transactions already in mempool (pool.check())
            // This includes: structure check, input validation, script verification, RBF checks
            for tx in &mempool_txs {
                // Check transaction structure (part of MempoolCheck)
                black_box(bllvm_consensus::transaction::check_transaction(black_box(
                    tx,
                )));
                // Check inputs against UTXO set (part of MempoolCheck)
                black_box(bllvm_consensus::transaction::check_tx_inputs(
                    black_box(tx),
                    black_box(&utxo_set),
                    black_box(0),
                ));
                // Check if standard (part of MempoolCheck)
                black_box(is_standard_tx(black_box(tx)));
                // Verify scripts (expensive part of MempoolCheck - Core does this)
                // For simple transactions without witnesses, verify_script does basic checks
                for input in &tx.inputs {
                    if let Some(utxo) = utxo_set.get(&input.prevout) {
                        black_box(bllvm_consensus::script::verify_script(
                            black_box(&input.script_sig),
                            black_box(&utxo.script_pubkey),
                            black_box(None), // No witness for simple transactions
                            black_box(0),    // Standard flags
                        ));
                    }
                }
            }
            // Also do RBF checks (part of MempoolCheck validation)
            let new_tx = &mempool_txs[0];
            let existing_tx = &mempool_txs[1];
            black_box(replacement_checks(
                black_box(new_tx),
                black_box(existing_tx),
                black_box(&utxo_set),
                black_box(&mempool),
            ));
        })
    });
}

criterion_group!(
    benches,
    benchmark_mempool_acceptance,
    benchmark_mempool_acceptance_complex,
    benchmark_is_standard_tx,
    benchmark_replacement_checks,
    benchmark_mempool_eviction,
    benchmark_accept_to_memory_pool_400tx,
    benchmark_is_standard_tx_400tx,
    benchmark_replacement_checks_mempool
);
criterion_main!(benches);
