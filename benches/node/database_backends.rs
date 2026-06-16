//! Storage backend benchmarks: redb, RocksDB, heed3/LMDB (rkyv zero-copy; default `auto` backend).
//!
//! Run:
//! ```text
//! # redb baseline only
//! cargo bench --features node-benches --bench database_backends
//!
//! # + RocksDB (multi_get_cf, fill_cache=false IBD path)
//! cargo bench --features node-benches-rocksdb --bench database_backends
//!
//! # + heed3 zero-copy (requires liblmdb)
//! cargo bench --features node-benches-heed3 --bench database_backends
//!
//! # all backends
//! cargo bench --features node-benches-storage --bench database_backends
//! ```
//!
//! Hot paths exercised:
//! - IBD cache-miss batch UTXO load (`get_many_no_cache` vs `get_many_heed3`)
//! - Live UTXO single-key get (`Tree::get` vs mmap single-key)
//! - Startup MuHash full-tree scan (`Tree::iter` vs `scan_heed3`)

mod database_backends_support;

use blvm_node::storage::database::Tree;
use blvm_node::storage::utxo_value_codec::{decode_utxo_with_codec, ValueCodec};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use database_backends_support::{key_refs, open_redb_utxo_tree};

#[cfg(feature = "node-benches-heed3")]
use blvm_node::storage::rkyv_codec::{access_utxo, utxo_from_archived};
#[cfg(feature = "node-benches-heed3")]
use database_backends_support::open_heed3_utxo_tree;
#[cfg(feature = "node-benches-rocksdb")]
use database_backends_support::open_rocksdb_utxo_tree;

const BATCH_SIZES: &[usize] = &[50, 500, 5000];
#[cfg(feature = "node-benches-heed3")]
const SCAN_SIZES: &[usize] = &[2_000, 10_000];

fn bench_utxo_batch_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_batch_read");
    group.sample_size(30);

    for &n in BATCH_SIZES {
        let redb = open_redb_utxo_tree(n).expect("redb fixture");
        group.bench_with_input(
            BenchmarkId::new("redb_get_many_owned", n),
            &redb,
            |b, fx| {
                let refs = key_refs(&fx.keys);
                b.iter(|| {
                    let values = fx.tree.get_many_no_cache(black_box(&refs)).unwrap();
                    for data in values.into_iter().flatten() {
                        let utxo = decode_utxo_with_codec(ValueCodec::Bincode, &data).unwrap();
                        black_box(utxo);
                    }
                });
            },
        );

        #[cfg(feature = "node-benches-rocksdb")]
        {
            let rocks = open_rocksdb_utxo_tree(n).expect("rocksdb fixture");
            group.bench_with_input(
                BenchmarkId::new("rocksdb_get_many_no_cache", n),
                &rocks,
                |b, fx| {
                    let refs = key_refs(&fx.keys);
                    b.iter(|| {
                        let values = fx.tree.get_many_no_cache(black_box(&refs)).unwrap();
                        for data in values.into_iter().flatten() {
                            let utxo = decode_utxo_with_codec(ValueCodec::Bincode, &data).unwrap();
                            black_box(utxo);
                        }
                    });
                },
            );
        }

        #[cfg(feature = "node-benches-heed3")]
        {
            let heed3 = open_heed3_utxo_tree(n).expect("heed3 fixture");
            group.bench_with_input(
                BenchmarkId::new("heed3_get_many_owned", n),
                &heed3,
                |b, fx| {
                    let refs = key_refs(&fx.keys);
                    b.iter(|| {
                        let values = fx.tree.get_many_no_cache(black_box(&refs)).unwrap();
                        for data in values.into_iter().flatten() {
                            let utxo = decode_utxo_with_codec(ValueCodec::Rkyv, &data).unwrap();
                            black_box(utxo);
                        }
                    });
                },
            );

            group.bench_with_input(
                BenchmarkId::new("heed3_get_many_zero_copy", n),
                &heed3,
                |b, fx| {
                    let heed3_tree = fx.tree.as_heed3_tree().unwrap();
                    let refs = key_refs(&fx.keys);
                    b.iter(|| {
                        let rtxn = heed3_tree.env().read_txn().unwrap();
                        let slices = heed3_tree
                            .get_many_heed3(black_box(&refs), black_box(&rtxn))
                            .unwrap();
                        for opt in slices {
                            if let Some(bytes) = opt {
                                let archived = access_utxo(bytes).unwrap();
                                black_box(utxo_from_archived(archived));
                            }
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_utxo_full_scan(c: &mut Criterion) {
    #[cfg(not(feature = "node-benches-heed3"))]
    {
        let _ = c;
        return;
    }

    #[cfg(feature = "node-benches-heed3")]
    {
        let mut group = c.benchmark_group("utxo_full_scan");
        group.sample_size(20);

        for &n in SCAN_SIZES {
            let fixture = open_heed3_utxo_tree(n).expect("heed3 scan fixture");
            let heed3_tree = fixture.tree.as_heed3_tree().unwrap();

            group.bench_with_input(
                BenchmarkId::new("iter_owned_bytes", n),
                &fixture,
                |b, fx| {
                    b.iter(|| {
                        let mut count = 0usize;
                        for row in fx.tree.iter() {
                            let (_k, v) = row.unwrap();
                            if access_utxo(black_box(&v)).is_ok() {
                                count += 1;
                            }
                        }
                        black_box(count);
                    });
                },
            );

            group.bench_with_input(
                BenchmarkId::new("scan_heed3_mmap", n),
                &heed3_tree,
                |b, ht| {
                    b.iter(|| {
                        let mut count = 0usize;
                        ht.scan_heed3(|_k, v| {
                            if access_utxo(black_box(v)).is_ok() {
                                count += 1;
                            }
                            Ok(())
                        })
                        .unwrap();
                        black_box(count);
                    });
                },
            );
        }
        group.finish();
    }
}

fn bench_single_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_single_get");
    group.sample_size(50);

    let redb = open_redb_utxo_tree(500).expect("redb fixture");
    let redb_key = redb.keys[250];
    group.bench_function("redb_get_owned", |b| {
        b.iter(|| {
            if let Some(data) = redb.tree.get(black_box(redb_key.as_slice())).unwrap() {
                let utxo = decode_utxo_with_codec(ValueCodec::Bincode, &data).unwrap();
                black_box(utxo);
            }
        });
    });

    #[cfg(feature = "node-benches-rocksdb")]
    {
        let rocks = open_rocksdb_utxo_tree(500).expect("rocksdb fixture");
        let rocks_key = rocks.keys[250];
        group.bench_function("rocksdb_get_no_cache", |b| {
            b.iter(|| {
                if let Some(data) = rocks.tree.get(black_box(rocks_key.as_slice())).unwrap() {
                    let utxo = decode_utxo_with_codec(ValueCodec::Bincode, &data).unwrap();
                    black_box(utxo);
                }
            });
        });
    }

    #[cfg(feature = "node-benches-heed3")]
    {
        let heed3 = open_heed3_utxo_tree(500).expect("heed3 fixture");
        let heed3_tree = heed3.tree.as_heed3_tree().unwrap();
        let heed3_key = heed3.keys[250];
        group.bench_function("heed3_get_zero_copy", |b| {
            b.iter(|| {
                let rtxn = heed3_tree.env().read_txn().unwrap();
                let slices = heed3_tree
                    .get_many_heed3(black_box(&[heed3_key.as_slice()]), black_box(&rtxn))
                    .unwrap();
                if let Some(Some(bytes)) = slices.into_iter().next() {
                    let archived = access_utxo(bytes).unwrap();
                    black_box(utxo_from_archived(archived));
                }
            });
        });
    }
    group.finish();
}

fn bench_utxo_batch_write(c: &mut Criterion) {
    const WRITE_SIZES: &[usize] = &[5_000, 20_000];
    let mut group = c.benchmark_group("utxo_batch_write");
    group.sample_size(15);

    for &n in WRITE_SIZES {
        let (keys, values) =
            database_backends_support::prepare_utxo_batch(n, ValueCodec::Bincode).expect("prepare");

        #[cfg(feature = "node-benches-rocksdb")]
        {
            let rocks = database_backends_support::open_empty_rocksdb_tree().expect("rocksdb");
            group.bench_with_input(
                BenchmarkId::new("rocksdb_batch_commit", n),
                &(rocks, &keys, &values),
                |b, (fx, keys, values)| {
                    b.iter(|| {
                        batch_write_chunked(
                            fx.tree.as_ref(),
                            keys,
                            values,
                            database_backends_support::WRITE_BENCH_CHUNK,
                        )
                        .unwrap();
                    });
                },
            );
        }

        {
            let redb = database_backends_support::open_empty_redb_tree().expect("redb");
            group.bench_with_input(
                BenchmarkId::new("redb_batch_commit", n),
                &(redb, &keys, &values),
                |b, (fx, keys, values)| {
                    b.iter(|| {
                        batch_write_chunked(
                            fx.tree.as_ref(),
                            keys,
                            values,
                            database_backends_support::WRITE_BENCH_CHUNK,
                        )
                        .unwrap();
                    });
                },
            );
        }

        #[cfg(feature = "node-benches-heed3")]
        {
            let (keys_r, values_r) =
                database_backends_support::prepare_utxo_batch(n, ValueCodec::Rkyv)
                    .expect("prepare rkyv");
            let heed3 = database_backends_support::open_empty_heed3_tree().expect("heed3");
            group.bench_with_input(
                BenchmarkId::new("heed3_batch_commit", n),
                &(heed3, &keys_r, &values_r),
                |b, (fx, keys, values)| {
                    b.iter(|| {
                        batch_write_chunked(
                            fx.tree.as_ref(),
                            keys,
                            values,
                            database_backends_support::WRITE_BENCH_CHUNK,
                        )
                        .unwrap();
                    });
                },
            );
        }

        let _ = (keys, values);
    }
    group.finish();
}

#[inline]
fn batch_write_chunked(
    tree: &dyn Tree,
    keys: &[blvm_node::storage::disk_utxo::OutPointKey],
    values: &[Vec<u8>],
    chunk_size: usize,
) -> anyhow::Result<()> {
    database_backends_support::batch_write_utxos(tree, keys, values, chunk_size)
}

criterion_group!(
    benches,
    bench_utxo_batch_reads,
    bench_utxo_batch_write,
    bench_utxo_full_scan,
    bench_single_get
);
criterion_main!(benches);
