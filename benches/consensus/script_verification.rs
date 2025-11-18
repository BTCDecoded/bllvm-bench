//! Script Verification Benchmarks
//! Measures script execution and verification performance

use bllvm_consensus::script::{eval_script, verify_script};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Create a simple script for verification
fn create_simple_script() -> Vec<u8> {
    vec![0x51, 0x51, 0x87] // OP_1 OP_1 OP_EQUAL
}

/// Create a complex script with many operations
fn create_complex_script() -> Vec<u8> {
    // Create a script with many operations (similar complexity to Core's VerifyNestedIfScript)
    let mut script = Vec::new();
    // Add many OP_DUP, OP_HASH160, OP_EQUALVERIFY operations
    for _ in 0..50 {
        script.push(0x76); // OP_DUP
        script.push(0xa9); // OP_HASH160
        script.push(0x14); // Push 20 bytes
        script.extend_from_slice(&[0x42; 20]);
        script.push(0x88); // OP_EQUALVERIFY
    }
    script.push(0xac); // OP_CHECKSIG
    script
}

fn benchmark_verify_script(c: &mut Criterion) {
    let script_sig = vec![0x51]; // OP_1
    let script_pubkey = create_simple_script();
    
    c.bench_function("verify_script", |b| {
        b.iter(|| {
            let result = verify_script(
                black_box(&script_sig),
                black_box(&script_pubkey),
                black_box(None), // No witness
                black_box(0),    // No flags
            );
            black_box(result)
        })
    });
}

fn benchmark_eval_script_complex(c: &mut Criterion) {
    let script = create_complex_script();
    
    c.bench_function("eval_script_complex", |b| {
        b.iter(|| {
            let mut stack = Vec::new();
            // Push some data for the script to operate on
            stack.push(vec![0x42; 20]);
            let result = eval_script(
                black_box(&script),
                black_box(&mut stack),
                black_box(0), // No flags
            );
            black_box(result)
        })
    });
}

criterion_group!(
    benches,
    benchmark_verify_script,
    benchmark_eval_script_complex
);
criterion_main!(benches);

