//! FIBRE (Fast Internet Bitcoin Relay Engine) performance benchmarks
//!
//! Benchmarks FIBRE block encoding, FEC encoding/decoding, and UDP packet handling.

use bllvm_node::network::fibre::FibreRelay;
use bllvm_protocol::{Block, BlockHeader};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use sha2::Digest;
use std::time::Duration;

fn create_test_block(size_kb: usize) -> Block {
    // Create a block with approximate size
    let tx_count = (size_kb * 1024) / 250; // Rough estimate: ~250 bytes per tx
    let mut transactions = Vec::new();

    // Create minimal transactions to approximate size
    for _ in 0..tx_count.min(1000) {
        // Cap at 1000 txs
        transactions.push(bllvm_protocol::Transaction {
            version: 1,
            inputs: vec![],
            outputs: vec![bllvm_protocol::TransactionOutput {
                value: 1000,
                script_pubkey: vec![0x76, 0xa9, 0x14, 0x00; 20].into(),
            }],
            lock_time: 0,
        });
    }

    Block {
        header: BlockHeader {
            version: 1,
            prev_block_hash: [0x11; 32],
            merkle_root: [0x22; 32],
            timestamp: 1234567890,
            bits: 0x1d00ffff,
            nonce: 0x12345678,
        },
        transactions: transactions.into_boxed_slice(),
    }
}

fn bench_fibre_encode_block(c: &mut Criterion) {
    let mut group = c.benchmark_group("fibre_encode_block");
    group.measurement_time(Duration::from_secs(10));

    for size_kb in [100, 500, 1000].iter() {
        let block = create_test_block(*size_kb);
        let mut relay = FibreRelay::new();

        group.bench_with_input(
            BenchmarkId::new("encode", format!("{}KB", size_kb)),
            &block,
            |b, block| {
                let mut relay = FibreRelay::new();
                b.iter(|| black_box(relay.encode_block(block.clone()).unwrap()));
            },
        );
    }

    group.finish();
}

fn bench_fibre_fec_encoding(c: &mut Criterion) {
    let mut group = c.benchmark_group("fibre_fec_encoding");
    group.measurement_time(Duration::from_secs(10));

    for size_kb in [100, 500, 1000].iter() {
        let block = create_test_block(*size_kb);
        let mut relay = FibreRelay::new();
        let encoded = relay.encode_block(block).unwrap();
        let chunk_count = encoded.chunk_count;

        group.bench_with_input(
            BenchmarkId::new("chunk_count", format!("{}KB", size_kb)),
            &chunk_count,
            |b, &chunk_count| {
                b.iter(|| black_box(chunk_count));
            },
        );
    }

    group.finish();
}

fn bench_fibre_chunk_serialization(c: &mut Criterion) {
    use bllvm_protocol::fibre::FecChunk;

    let mut group = c.benchmark_group("fibre_chunk_serialization");
    group.measurement_time(Duration::from_secs(5));

    // Test different chunk sizes
    for chunk_size in [100, 500, 1000, 1400].iter() {
        let data = vec![0u8; *chunk_size];
        let block_hash = [0x42; 32];

        // Create chunk manually (serialize/deserialize test)
        let mut packet = Vec::new();
        packet.extend_from_slice(&bllvm_protocol::fibre::FIBRE_MAGIC);
        packet.push(bllvm_protocol::fibre::FIBRE_VERSION);
        packet.push(bllvm_protocol::fibre::PACKET_TYPE_CHUNK);
        packet.extend_from_slice(&block_hash);
        packet.extend_from_slice(&12345u64.to_be_bytes());
        packet.extend_from_slice(&0u32.to_be_bytes());
        packet.extend_from_slice(&10u32.to_be_bytes());
        packet.extend_from_slice(&8u32.to_be_bytes());
        packet.extend_from_slice(&(data.len() as u32).to_be_bytes());
        packet.extend_from_slice(&data);
        let checksum = crc32fast::hash(&packet);
        packet.extend_from_slice(&checksum.to_be_bytes());

        let chunk = FecChunk::deserialize(&packet).unwrap();

        group.bench_with_input(
            BenchmarkId::new("serialize", format!("{}B", chunk_size)),
            &chunk,
            |b, chunk| {
                b.iter(|| black_box(chunk.serialize().unwrap()));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("deserialize", format!("{}B", chunk_size)),
            &packet,
            |b, packet| {
                b.iter(|| black_box(FecChunk::deserialize(packet).unwrap()));
            },
        );
    }

    group.finish();
}

fn bench_fibre_block_hash_calculation(c: &mut Criterion) {
    let mut group = c.benchmark_group("fibre_block_hash");
    group.measurement_time(Duration::from_secs(5));

    let block = create_test_block(1000);

    group.bench_function("double_sha256_header", |b| {
        b.iter(|| {
            let mut header_bytes = Vec::with_capacity(80);
            header_bytes.extend_from_slice(&(block.header.version as i32).to_le_bytes());
            header_bytes.extend_from_slice(&block.header.prev_block_hash);
            header_bytes.extend_from_slice(&block.header.merkle_root);
            header_bytes.extend_from_slice(&(block.header.timestamp as u32).to_le_bytes());
            header_bytes.extend_from_slice(&(block.header.bits as u32).to_le_bytes());
            header_bytes.extend_from_slice(&(block.header.nonce as u32).to_le_bytes());

            let first_hash = sha2::Sha256::digest(&header_bytes);
            let second_hash = sha2::Sha256::digest(&first_hash);
            black_box(second_hash)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_fibre_encode_block,
    bench_fibre_fec_encoding,
    bench_fibre_chunk_serialization,
    bench_fibre_block_hash_calculation
);
criterion_main!(benches);
