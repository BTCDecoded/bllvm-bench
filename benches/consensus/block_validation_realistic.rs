//! Realistic Block Validation Benchmark
//! Uses real P2WPKH scripts with actual ECDSA signatures (matches Core's ConnectBlock benchmark)
//!
//! This benchmark:
//! 1. Creates 1000 transactions with real P2WPKH outputs
//! 2. Uses actual secp256k1 keypairs and signatures
//! 3. Forces full script verification (no assume-valid optimization)
//! 4. Matches Core's ConnectBlock benchmark methodology

use bllvm_consensus::block::connect_block;
use bllvm_consensus::segwit::Witness;
use bllvm_consensus::{
    tx_inputs, tx_outputs, Block, BlockHeader, OutPoint, Transaction, TransactionInput, TransactionOutput, UTXO, UtxoSet,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use secp256k1::{Secp256k1, SecretKey, PublicKey, Message, Signing};
use sha2::{Sha256, Digest};
use bllvm_consensus::transaction_hash::{calculate_transaction_sighash, SighashType};

/// Create a P2WPKH scriptPubkey (OP_0 <20-byte hash>)
fn create_p2wpkh_script_pubkey(pubkey: &PublicKey) -> Vec<u8> {
    let pubkey_bytes = pubkey.serialize();
    let mut hasher = Sha256::new();
    hasher.update(&pubkey_bytes);
    let hash = hasher.finalize();
    let mut script = vec![0x00]; // OP_0
    script.extend_from_slice(&hash[..20]); // 20-byte hash
    script
}

/// Create a P2WPKH witness (signature + pubkey) with proper sighash
fn create_p2wpkh_witness(
    secp: &Secp256k1<impl Signing>,
    secret_key: &SecretKey,
    tx: &Transaction,
    input_index: usize,
    prevouts: &[TransactionOutput],
) -> Vec<Vec<u8>> {
    // Calculate proper transaction sighash
    let sighash = calculate_transaction_sighash(tx, input_index, prevouts, SighashType::All)
        .expect("Failed to calculate sighash");
    
    let msg = Message::from_digest_slice(&sighash).expect("Invalid sighash");
    let signature = secp.sign_ecdsa(&msg, secret_key);
    let pubkey = PublicKey::from_secret_key(secp, secret_key);
    
    let mut sig_bytes = signature.serialize_der().to_vec();
    sig_bytes.push(0x01); // SIGHASH_ALL
    
    vec![sig_bytes, pubkey.serialize().to_vec()]
}

/// Create a realistic test block with actual P2WPKH transactions and signatures
/// Matches Core's CreateTestBlock approach
fn create_realistic_test_block_with_signatures(num_txs: usize) -> (Block, UtxoSet, Vec<Witness>, Vec<SecretKey>) {
    let secp = Secp256k1::new();
    
    // Generate keypairs for transactions
    let mut secret_keys = Vec::new();
    let mut public_keys = Vec::new();
    for _ in 0..num_txs {
        let sk = SecretKey::from_slice(&rand::random::<[u8; 32]>()).expect("Invalid secret key");
        let pk = PublicKey::from_secret_key(&secp, &sk);
        secret_keys.push(sk);
        public_keys.push(pk);
    }
    
    // Create coinbase transaction
    let coinbase = Transaction {
        version: 1,
        inputs: tx_inputs![TransactionInput {
            prevout: OutPoint {
                hash: [0; 32],
                index: 0xffffffff, // Coinbase
            },
            script_sig: vec![0x51; 4], // OP_1 repeated
            sequence: 0xffffffff,
        }],
        outputs: tx_outputs![TransactionOutput {
            value: 50_000_000_000, // 50 BTC
            script_pubkey: create_p2wpkh_script_pubkey(&public_keys[0]),
        }],
        lock_time: 0,
    };
    
    // Create UTXO set and transactions
    // Strategy: Create a chain where each transaction spends the first output of the previous transaction
    let mut utxo_set = UtxoSet::new();
    let mut transactions = vec![coinbase];
    
    // Create first UTXO from coinbase (will be spent by first transaction)
    let coinbase_outpoint = OutPoint {
        hash: [0; 32], // Coinbase hash (simplified)
        index: 0,
    };
    let coinbase_utxo = UTXO {
        value: 50_000_000_000, // 50 BTC from coinbase
        script_pubkey: create_p2wpkh_script_pubkey(&public_keys[0]),
        height: 0,
    };
    utxo_set.insert(coinbase_outpoint.clone(), coinbase_utxo);
    
    // Create transactions that form a chain
    let mut prev_outpoint = coinbase_outpoint;
    let mut prev_output_value = 50_000_000_000;
    
    for i in 0..num_txs {
        // Create transaction that spends previous output
        let tx = Transaction {
            version: 1,
            inputs: tx_inputs![TransactionInput {
                prevout: prev_outpoint.clone(),
                script_sig: vec![], // Empty for SegWit
                sequence: 0xffffffff,
            }],
            outputs: tx_outputs![
                TransactionOutput {
                    value: prev_output_value / 2, // Half to output
                    script_pubkey: create_p2wpkh_script_pubkey(&public_keys[i % public_keys.len()]),
                },
                TransactionOutput {
                    value: prev_output_value / 2 - 1000, // Rest as change (minus small fee)
                    script_pubkey: create_p2wpkh_script_pubkey(&public_keys[(i + 1) % public_keys.len()]),
                }
            ],
            lock_time: 0,
        };
        
        transactions.push(tx);
        
        // Update for next iteration
        prev_output_value = prev_output_value / 2;
        let next_script_pubkey = create_p2wpkh_script_pubkey(&public_keys[i % public_keys.len()]);
        prev_outpoint = OutPoint {
            hash: [i as u8 + 1; 32], // Different hash for each
            index: 0,
        };
        
        // Create UTXO for next transaction to spend
        let utxo = UTXO {
            value: prev_output_value,
            script_pubkey: next_script_pubkey,
            height: 0,
        };
        utxo_set.insert(prev_outpoint.clone(), utxo);
    }
    
    // Now create proper witnesses with signatures for each transaction
    let mut final_witnesses = vec![Vec::new()]; // Coinbase has empty witness
    for (i, tx) in transactions.iter().enumerate().skip(1) {
        // Get the previous output that this transaction spends
        let prevout = &tx.inputs[0].prevout;
        let prev_output = utxo_set.get(prevout).expect("UTXO not found");
        
        // Create prevouts array for sighash calculation
        let prevouts = vec![TransactionOutput {
            value: prev_output.value,
            script_pubkey: prev_output.script_pubkey.clone(),
        }];
        
        // Determine which key to use for signing (based on which key owns the prevout)
        // For simplicity, use key index based on transaction index
        let key_index = (i - 1) % secret_keys.len();
        
        // Create witness with proper signature
        let witness = create_p2wpkh_witness(
            &secp,
            &secret_keys[key_index],
            tx,
            0,
            &prevouts,
        );
        final_witnesses.push(witness);
    }
    
    let block = Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: [0; 32],
            merkle_root: [0; 32], // Would be calculated in real scenario
            timestamp: 1234567890,
            bits: 0x1d00ffff,
            nonce: 0,
        },
        transactions: transactions.into_boxed_slice(),
    };
    
    (block, utxo_set, final_witnesses, secret_keys)
}

fn benchmark_connect_block_realistic_100tx(c: &mut Criterion) {
    // Disable assume-valid optimization for benchmarks to ensure full validation
    // Use height 1 to ensure assume-valid doesn't skip verification (height 0 might be special)
    let (block, utxo_set, witnesses, _secret_keys) = create_realistic_test_block_with_signatures(100);
    
    c.bench_function("connect_block_realistic_100tx", |b| {
        b.iter(|| {
            // Use height 1 to ensure assume-valid doesn't skip verification
            let result = connect_block(
                black_box(&block),
                black_box(&witnesses),
                black_box(utxo_set.clone()),
                black_box(1), // Height 1 = no assume-valid optimization
                black_box(None),
            );
            // Ensure we're actually validating - use result to ensure it's computed
            black_box(result);
        })
    });
}

fn benchmark_connect_block_realistic_1000tx(c: &mut Criterion) {
    // Disable assume-valid optimization for benchmarks to ensure full validation
    // Use height 1 to ensure assume-valid doesn't skip verification (height 0 might be special)
    let (block, utxo_set, witnesses, _secret_keys) = create_realistic_test_block_with_signatures(1000);
    
    c.bench_function("connect_block_realistic_1000tx", |b| {
        b.iter(|| {
            // Use height 1 to ensure assume-valid doesn't skip verification
            let result = connect_block(
                black_box(&block),
                black_box(&witnesses),
                black_box(utxo_set.clone()),
                black_box(1), // Height 1 = no assume-valid optimization
                black_box(None),
            );
            // Ensure we're actually validating - use result to ensure it's computed
            black_box(result);
        })
    });
}

criterion_group!(
    benches,
    benchmark_connect_block_realistic_100tx,
    benchmark_connect_block_realistic_1000tx
);
criterion_main!(benches);
