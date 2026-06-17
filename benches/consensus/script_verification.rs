//! Script Verification Benchmarks
//! Measures script execution and verification performance

use blvm_protocol::script::{SigVersion, eval_script, to_stack_element, verify_script};
use criterion::{Criterion, black_box, criterion_group, criterion_main};

/// Create a simple script for verification
fn create_simple_script() -> Vec<u8> {
    vec![
        blvm_protocol::opcodes::OP_1,
        blvm_protocol::opcodes::OP_1,
        blvm_protocol::opcodes::OP_EQUAL,
    ]
}

/// Create a complex script with many operations
/// Matches Core's VerifyNestedIfScript complexity: 100 nested operations + 1000 operations inside
/// Since Commons doesn't have OP_IF yet, we use OP_DUP/OP_HASH160/OP_EQUALVERIFY pattern
/// to achieve similar operation count: 100 * 4 ops = 400 ops + 1000 ops = 1400 total ops
fn create_complex_script() -> Vec<u8> {
    // Create a script with many operations (matches Core's VerifyNestedIfScript operation count)
    let mut script = Vec::new();
    // Core: 100 nested IF + 1000 OP_1 + 100 ENDIF = ~1200 operations
    // We use: 100 * (OP_DUP + OP_HASH160 + push + OP_EQUALVERIFY) = 400 ops
    // Plus 1000 OP_1 operations = 1400 total (slightly more to account for no IF overhead)
    for _ in 0..100 {
        script.push(blvm_protocol::opcodes::OP_DUP);
        script.push(blvm_protocol::opcodes::OP_HASH160);
        script.push(blvm_protocol::opcodes::PUSH_20_BYTES);
        script.extend_from_slice(&[0x42; 20]);
        script.push(blvm_protocol::opcodes::OP_EQUALVERIFY);
    }
    // Add 1000 OP_1 operations (matches Core's inner loop)
    for _ in 0..1000 {
        script.push(blvm_protocol::opcodes::OP_1);
    }
    script.push(blvm_protocol::opcodes::OP_CHECKSIG);
    script
}

fn benchmark_verify_script(c: &mut Criterion) {
    let script_sig = vec![blvm_protocol::opcodes::OP_1];
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
            stack.push(to_stack_element(&[0x42; 20]));
            let result = eval_script(
                black_box(&script),
                black_box(&mut stack),
                black_box(0),
                SigVersion::Base,
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
