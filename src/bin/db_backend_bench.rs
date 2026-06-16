//! Quick storage backend timing report (no Criterion harness).
//!
//! ```text
//! cargo run --release --features node-benches --bin db_backend_bench
//! cargo run --release --features node-benches-storage --bin db_backend_bench -- --entries 5000
//! ```

use anyhow::{Context, Result};
use blvm_node::storage::utxo_value_codec::{decode_utxo_with_codec, ValueCodec};
use clap::Parser;
use std::time::Instant;

#[path = "../../benches/node/database_backends_support.rs"]
mod database_backends_support;

use database_backends_support::{
    batch_write_utxos, key_refs, open_empty_redb_tree, open_redb_utxo_tree, prepare_utxo_batch,
    WRITE_BENCH_CHUNK,
};

#[cfg(feature = "node-benches-heed3")]
use blvm_node::storage::database::Tree;
#[cfg(feature = "node-benches-heed3")]
use blvm_node::storage::rkyv_codec::{access_utxo, utxo_from_archived};
#[cfg(feature = "node-benches-heed3")]
use database_backends_support::open_empty_heed3_tree;
#[cfg(feature = "node-benches-heed3")]
use database_backends_support::open_heed3_utxo_tree;
#[cfg(feature = "node-benches-rocksdb")]
use database_backends_support::{open_empty_rocksdb_tree, open_rocksdb_utxo_tree};

#[derive(Parser, Debug)]
#[command(name = "db_backend_bench")]
struct Args {
    /// Number of UTXO rows to populate before timing batch reads.
    #[arg(long, default_value_t = 500)]
    entries: usize,
    /// Iterations per scenario.
    #[arg(long, default_value_t = 50)]
    iters: usize,
}

fn time_batch(label: &str, iters: usize, mut f: impl FnMut()) -> f64 {
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let total_ms = start.elapsed().as_secs_f64() * 1000.0;
    let per_iter = total_ms / iters as f64;
    println!("{label:40} {per_iter:8.3} ms/iter ({iters} iters, {total_ms:.1} ms total)");
    per_iter
}

fn main() -> Result<()> {
    let args = Args::parse();
    let n = args.entries;
    let iters = args.iters;

    println!("db_backend_bench: entries={n} iters={iters}\n");

    let redb = open_redb_utxo_tree(n).context("open redb fixture")?;
    let refs = key_refs(&redb.keys);
    time_batch("redb/get_many_owned", iters, || {
        let values = redb.tree.get_many_no_cache(&refs).unwrap();
        for data in values.into_iter().flatten() {
            black_box(decode_utxo_with_codec(ValueCodec::Bincode, &data).unwrap());
        }
    });

    {
        let write_n = n.min(20_000);
        let (keys, values) =
            prepare_utxo_batch(write_n, ValueCodec::Bincode).context("write batch redb")?;
        let redb = open_empty_redb_tree().context("empty redb")?;
        time_batch(
            &format!("redb/batch_write/{write_n}"),
            iters.min(10),
            || {
                batch_write_utxos(redb.tree.as_ref(), &keys, &values, WRITE_BENCH_CHUNK).unwrap();
            },
        );
    }

    #[cfg(feature = "node-benches-rocksdb")]
    {
        let rocks = open_rocksdb_utxo_tree(n).context("open rocksdb fixture")?;
        let refs = key_refs(&rocks.keys);
        time_batch("rocksdb/get_many_no_cache", iters, || {
            let values = rocks.tree.get_many_no_cache(&refs).unwrap();
            for data in values.into_iter().flatten() {
                black_box(decode_utxo_with_codec(ValueCodec::Bincode, &data).unwrap());
            }
        });
    }

    #[cfg(feature = "node-benches-heed3")]
    {
        let heed3 = open_heed3_utxo_tree(n).context("open heed3 fixture")?;
        let heed3_tree = Tree::as_heed3_tree(heed3.tree.as_ref()).context("heed3 downcast")?;
        let refs = key_refs(&heed3.keys);

        time_batch("heed3/get_many_owned", iters, || {
            let values = heed3.tree.get_many_no_cache(&refs).unwrap();
            for data in values.into_iter().flatten() {
                black_box(decode_utxo_with_codec(ValueCodec::Rkyv, &data).unwrap());
            }
        });

        time_batch("heed3/get_many_zero_copy", iters, || {
            let rtxn = heed3_tree.env().read_txn().unwrap();
            let slices = heed3_tree.get_many_heed3(&refs, &rtxn).unwrap();
            for opt in slices {
                if let Some(bytes) = opt {
                    let archived = access_utxo(bytes).unwrap();
                    black_box(utxo_from_archived(archived));
                }
            }
        });

        let scan_n = n.min(10_000);
        if scan_n >= 100 {
            let scan_fixture = open_heed3_utxo_tree(scan_n).context("heed3 scan fixture")?;
            let scan_tree =
                Tree::as_heed3_tree(scan_fixture.tree.as_ref()).context("heed3 scan downcast")?;
            let scan_iters = iters.min(20);

            time_batch(
                &format!("heed3/scan_iter_owned/{scan_n}"),
                scan_iters,
                || {
                    let mut count = 0usize;
                    for row in scan_fixture.tree.iter() {
                        let (_k, v) = row.unwrap();
                        if access_utxo(&v).is_ok() {
                            count += 1;
                        }
                    }
                    black_box(count);
                },
            );

            time_batch(
                &format!("heed3/scan_heed3_mmap/{scan_n}"),
                scan_iters,
                || {
                    let mut count = 0usize;
                    scan_tree
                        .scan_heed3(|_k, v| {
                            if access_utxo(v).is_ok() {
                                count += 1;
                            }
                            Ok(())
                        })
                        .unwrap();
                    black_box(count);
                },
            );
        }
    }

    {
        let write_n = n.min(20_000);
        let (keys, values) =
            prepare_utxo_batch(write_n, ValueCodec::Bincode).context("write batch redb")?;
        let redb = open_empty_redb_tree().context("empty redb")?;
        time_batch(
            &format!("redb/batch_write/{write_n}"),
            iters.min(10),
            || {
                batch_write_utxos(redb.tree.as_ref(), &keys, &values, WRITE_BENCH_CHUNK).unwrap();
            },
        );
    }

    #[cfg(feature = "node-benches-rocksdb")]
    {
        let write_n = n.min(20_000);
        let (keys, values) =
            prepare_utxo_batch(write_n, ValueCodec::Bincode).context("write batch")?;
        let rocks = open_empty_rocksdb_tree().context("empty rocksdb")?;
        time_batch(
            &format!("rocksdb/batch_write/{write_n}"),
            iters.min(10),
            || {
                batch_write_utxos(rocks.tree.as_ref(), &keys, &values, WRITE_BENCH_CHUNK).unwrap();
            },
        );
    }

    #[cfg(feature = "node-benches-heed3")]
    {
        let write_n = n.min(20_000);
        let (keys, values) =
            prepare_utxo_batch(write_n, ValueCodec::Rkyv).context("write batch rkyv")?;
        let heed3 = open_empty_heed3_tree().context("empty heed3")?;
        time_batch(
            &format!("heed3/batch_write/{write_n}"),
            iters.min(10),
            || {
                batch_write_utxos(heed3.tree.as_ref(), &keys, &values, WRITE_BENCH_CHUNK).unwrap();
            },
        );
    }

    #[cfg(not(feature = "node-benches-rocksdb"))]
    eprintln!("note: add --features node-benches-rocksdb for RocksDB timings");
    #[cfg(not(feature = "node-benches-heed3"))]
    eprintln!("note: add --features node-benches-heed3 for heed3/LMDB timings");

    Ok(())
}

#[inline(never)]
fn black_box<T>(v: T) -> T {
    v
}
