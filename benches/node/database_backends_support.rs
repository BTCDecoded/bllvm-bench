//! Shared fixtures for storage backend benchmarks (redb / RocksDB / heed3).

use anyhow::Result;
use blvm_node::storage::database::{create_database, Database, DatabaseBackend, Tree};
use blvm_node::storage::disk_utxo::OutPointKey;
use blvm_node::storage::utxo_value_codec::{encode_utxo_with_codec, ValueCodec};
use blvm_node::UTXO;
use std::sync::Arc;
use tempfile::TempDir;

/// Chunk size for write benchmarks (similar to IBD flush sub-batches).
pub const WRITE_BENCH_CHUNK: usize = 1_000;

/// Pre-generate keys and encoded UTXO bytes for write benchmarks.
pub fn prepare_utxo_batch(n: usize, codec: ValueCodec) -> Result<(Vec<OutPointKey>, Vec<Vec<u8>>)> {
    let mut keys = Vec::with_capacity(n);
    let mut values = Vec::with_capacity(n);
    for i in 0..n as u64 {
        let key = key_for_index(i);
        let bytes = encode_utxo_with_codec(codec, &sample_utxo(i))?;
        keys.push(key);
        values.push(bytes);
    }
    Ok((keys, values))
}

/// Write UTXO rows in chunked batches (simulates IBD flush batch commits).
pub fn batch_write_utxos(
    tree: &dyn Tree,
    keys: &[OutPointKey],
    values: &[Vec<u8>],
    chunk_size: usize,
) -> Result<()> {
    debug_assert_eq!(keys.len(), values.len());
    let mut offset = 0usize;
    while offset < keys.len() {
        let end = (offset + chunk_size).min(keys.len());
        let mut batch = tree.batch()?;
        for i in offset..end {
            batch.put(keys[i].as_slice(), &values[i]);
        }
        batch.commit_no_wal()?;
        offset = end;
    }
    Ok(())
}

pub fn open_empty_redb_tree() -> Result<UtxoTreeFixture> {
    open_redb_utxo_tree(0)
}

#[cfg(feature = "node-benches-rocksdb")]
pub fn open_empty_rocksdb_tree() -> Result<UtxoTreeFixture> {
    let dir = TempDir::new()?;
    let db: Arc<dyn Database> =
        Arc::from(create_database(dir.path(), DatabaseBackend::RocksDB, None)?);
    let tree: Arc<dyn Tree> = Arc::from(db.open_tree("ibd_utxos")?);
    Ok(UtxoTreeFixture {
        _dir: dir,
        tree,
        keys: Vec::new(),
    })
}

#[cfg(feature = "node-benches-heed3")]
pub fn open_empty_heed3_tree() -> Result<UtxoTreeFixture> {
    let dir = TempDir::new()?;
    let db: Arc<dyn Database> =
        Arc::from(create_database(dir.path(), DatabaseBackend::Heed3, None)?);
    let tree: Arc<dyn Tree> = Arc::from(db.open_tree("ibd_utxos")?);
    Ok(UtxoTreeFixture {
        _dir: dir,
        tree,
        keys: Vec::new(),
    })
}

/// Prepared UTXO tree: N rows written, plus the key list used for batch lookups.
pub struct UtxoTreeFixture {
    pub _dir: TempDir,
    pub tree: Arc<dyn Tree>,
    pub keys: Vec<OutPointKey>,
}

fn sample_utxo(i: u64) -> UTXO {
    UTXO {
        value: (i as i64).saturating_mul(5_000),
        script_pubkey: vec![0x76, 0xa9, 0x14, (i & 0xff) as u8].into(),
        height: i.saturating_mul(10),
        is_coinbase: i % 100 == 0,
    }
}

fn key_for_index(i: u64) -> OutPointKey {
    let mut key = [0u8; 40];
    key[..8].copy_from_slice(&i.to_be_bytes());
    key
}

fn populate_tree(tree: &dyn Tree, n: usize, codec: ValueCodec) -> Result<Vec<OutPointKey>> {
    let mut keys = Vec::with_capacity(n);
    for i in 0..n as u64 {
        let key = key_for_index(i);
        let bytes = encode_utxo_with_codec(codec, &sample_utxo(i))?;
        tree.insert(key.as_slice(), &bytes)?;
        keys.push(key);
    }
    Ok(keys)
}

pub fn open_redb_utxo_tree(n: usize) -> Result<UtxoTreeFixture> {
    let dir = TempDir::new()?;
    let db: Arc<dyn Database> =
        Arc::from(create_database(dir.path(), DatabaseBackend::Redb, None)?);
    let tree: Arc<dyn Tree> = Arc::from(db.open_tree("ibd_utxos")?);
    let keys = populate_tree(tree.as_ref(), n, ValueCodec::Bincode)?;
    Ok(UtxoTreeFixture {
        _dir: dir,
        tree,
        keys,
    })
}

#[cfg(feature = "node-benches-rocksdb")]
pub fn open_rocksdb_utxo_tree(n: usize) -> Result<UtxoTreeFixture> {
    let dir = TempDir::new()?;
    let db: Arc<dyn Database> =
        Arc::from(create_database(dir.path(), DatabaseBackend::RocksDB, None)?);
    let tree: Arc<dyn Tree> = Arc::from(db.open_tree("ibd_utxos")?);
    let keys = populate_tree(tree.as_ref(), n, ValueCodec::Bincode)?;
    Ok(UtxoTreeFixture {
        _dir: dir,
        tree,
        keys,
    })
}

#[cfg(feature = "node-benches-heed3")]
pub fn open_heed3_utxo_tree(n: usize) -> Result<UtxoTreeFixture> {
    let dir = TempDir::new()?;
    let db: Arc<dyn Database> =
        Arc::from(create_database(dir.path(), DatabaseBackend::Heed3, None)?);
    let tree: Arc<dyn Tree> = Arc::from(db.open_tree("ibd_utxos")?);
    let keys = populate_tree(tree.as_ref(), n, ValueCodec::Rkyv)?;
    Ok(UtxoTreeFixture {
        _dir: dir,
        tree,
        keys,
    })
}

pub fn key_refs(keys: &[OutPointKey]) -> Vec<&[u8]> {
    keys.iter().map(|k| k.as_slice()).collect()
}
