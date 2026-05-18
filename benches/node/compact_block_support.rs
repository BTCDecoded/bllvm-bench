//! Local BIP152 compact-block helpers for benches (avoids linking full `blvm-node`).
//! Wire types match [`blvm_protocol::bip152`].
use blvm_protocol::{Block, Hash, Transaction};
use sha2::{Digest, Sha256};
use siphasher::sip::SipHasher24;
use std::collections::HashSet;
use std::hash::Hasher;

pub use blvm_protocol::bip152::{CompactBlock, ShortTxId};

fn encode_varint(value: u64) -> Vec<u8> {
    if value < 0xfd {
        vec![value as u8]
    } else if value <= 0xffff {
        let mut result = vec![0xfd];
        result.extend_from_slice(&(value as u16).to_le_bytes());
        result
    } else if value <= 0xffffffff {
        let mut result = vec![0xfe];
        result.extend_from_slice(&(value as u32).to_le_bytes());
        result
    } else {
        let mut result = vec![0xff];
        result.extend_from_slice(&value.to_le_bytes());
        result
    }
}

pub fn calculate_tx_hash(tx: &Transaction) -> Hash {
    let mut data = Vec::new();
    data.extend_from_slice(&(tx.version as u32).to_le_bytes());
    data.extend_from_slice(&encode_varint(tx.inputs.len() as u64));
    for input in &tx.inputs {
        data.extend_from_slice(&input.prevout.hash);
        data.extend_from_slice(&input.prevout.index.to_le_bytes());
        data.extend_from_slice(&encode_varint(input.script_sig.len() as u64));
        data.extend_from_slice(&input.script_sig);
        data.extend_from_slice(&(input.sequence as u32).to_le_bytes());
    }
    data.extend_from_slice(&encode_varint(tx.outputs.len() as u64));
    for output in &tx.outputs {
        data.extend_from_slice(&(output.value as u64).to_le_bytes());
        data.extend_from_slice(&encode_varint(output.script_pubkey.len() as u64));
        data.extend_from_slice(&output.script_pubkey);
    }
    data.extend_from_slice(&(tx.lock_time as u32).to_le_bytes());
    let hash1 = Sha256::digest(&data);
    let hash2 = Sha256::digest(hash1);
    let mut result = [0u8; 32];
    result.copy_from_slice(&hash2);
    result
}

pub fn calculate_short_tx_id(tx_hash: &Hash, nonce: u64) -> ShortTxId {
    let k0 = nonce;
    let k1 = nonce.wrapping_add(1);
    let mut hasher = SipHasher24::new_with_keys(k0, k1);
    hasher.write(tx_hash);
    let hash_result = hasher.finish();
    let mut short_id = [0u8; 6];
    short_id.copy_from_slice(&hash_result.to_le_bytes()[..6]);
    short_id
}

pub fn create_compact_block(
    block: &Block,
    nonce: u64,
    prefilled_indices: &HashSet<usize>,
) -> CompactBlock {
    let mut short_ids = Vec::new();
    let mut prefilled_txs = Vec::new();
    for (index, tx) in block.transactions.iter().enumerate() {
        if prefilled_indices.contains(&index) {
            prefilled_txs.push((index, tx.clone()));
        } else {
            let tx_hash = calculate_tx_hash(tx);
            short_ids.push(calculate_short_tx_id(&tx_hash, nonce));
        }
    }
    CompactBlock {
        header: block.header.clone(),
        nonce,
        short_ids,
        prefilled_txs,
    }
}
