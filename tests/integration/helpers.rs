//! Test helpers for differential testing

use bllvm_consensus::{
    tx_inputs, tx_outputs, Block, BlockHeader, OutPoint, Transaction, TransactionInput,
    TransactionOutput,
};
use bllvm_consensus::types::Network;

/// Create a test block with coinbase transaction
pub fn create_test_block(height: u64) -> Block {
    // Create coinbase transaction with BIP34 height
    let mut coinbase_script = vec![0x03]; // OP_PUSH_3 (for height encoding)
    coinbase_script.extend_from_slice(&height.to_le_bytes()[..3]);
    coinbase_script.push(0x51); // OP_1

    let coinbase = Transaction {
        version: 1,
        inputs: tx_inputs![TransactionInput {
            prevout: OutPoint {
                hash: [0; 32],
                index: 0xffffffff, // Coinbase
            },
            script_sig: coinbase_script,
            sequence: 0xffffffff,
        }],
        outputs: tx_outputs![TransactionOutput {
            value: 50_000_000_000, // 50 BTC
            script_pubkey: vec![0x51], // OP_1
        }],
        lock_time: 0,
    };

    Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: [0; 32],
            merkle_root: [0; 32], // Would need to calculate actual merkle root
            timestamp: 1234567890 + height,
            bits: 0x1d00ffff,
            nonce: 0,
        },
        transactions: vec![coinbase].into_boxed_slice(),
    }
}

/// Create a block violating BIP30 (duplicate coinbase)
pub fn create_bip30_violation_block(height: u64) -> Block {
    let block = create_test_block(height);
    // Duplicate the coinbase transaction (violates BIP30)
    let mut transactions = block.transactions.to_vec();
    transactions.push(transactions[0].clone());
    Block {
        transactions: transactions.into_boxed_slice(),
        ..block
    }
}

/// Create a block violating BIP34 (missing height in coinbase)
pub fn create_bip34_violation_block(height: u64) -> Block {
    // Create coinbase without height encoding
    let coinbase = Transaction {
        version: 1,
        inputs: tx_inputs![TransactionInput {
            prevout: OutPoint {
                hash: [0; 32],
                index: 0xffffffff,
            },
            script_sig: vec![0x51], // Just OP_1, no height
            sequence: 0xffffffff,
        }],
        outputs: tx_outputs![TransactionOutput {
            value: 50_000_000_000,
            script_pubkey: vec![0x51],
        }],
        lock_time: 0,
    };

    Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: [0; 32],
            merkle_root: [0; 32],
            timestamp: 1234567890 + height,
            bits: 0x1d00ffff,
            nonce: 0,
        },
        transactions: vec![coinbase].into_boxed_slice(),
    }
}

/// Create a block violating BIP90 (invalid block version)
pub fn create_bip90_violation_block(height: u64, invalid_version: i32) -> Block {
    let mut block = create_test_block(height);
    block.header.version = invalid_version;
    block
}

/// Validate block with BLLVM
pub fn validate_bllvm_block(block: &Block, height: u64, network: Network) -> bllvm_consensus::block::BlockValidationResult {
    use bllvm_consensus::block::connect_block;
    use bllvm_consensus::segwit::Witness;
    use bllvm_consensus::UtxoSet;

    let witnesses: Vec<Witness> = block.transactions.iter().map(|_| Vec::new()).collect();
    let utxo_set = UtxoSet::new();
    connect_block(block, &witnesses, utxo_set, height, None, network)
}

