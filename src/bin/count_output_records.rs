//! Quick diagnostic: count variable-length OutputRef records in a binary file.
//! Usage: count_output_records <path>
//! Streams through without loading into memory; reports total and any leftover bytes.

use blvm_bench::sort_merge::output_refs::OutputRef;
use std::fs::File;
use std::io::{BufReader, Read};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        "/mnt/extra/blockchain/sort_merge_data/outputs_unsorted.bin".to_string()
    });

    println!("Counting OutputRef records in: {}", path);

    let file_len = std::fs::metadata(&path)?.len();
    println!(
        "File size: {:.3} GB ({} bytes)",
        file_len as f64 / 1_073_741_824.0,
        file_len
    );

    let file = File::open(&path)?;
    let mut reader = BufReader::with_capacity(64 * 1024 * 1024, file);

    let mut buf = vec![0u8; 512 * 1024];
    let mut leftover: Vec<u8> = Vec::with_capacity(1024 * 1024);
    let mut count = 0u64;
    let mut bytes_consumed = 0u64;
    let start = std::time::Instant::now();
    let mut last_report = std::time::Instant::now();
    let mut eof = false;

    loop {
        match OutputRef::from_bytes(&leftover) {
            Some((_rec, consumed)) => {
                leftover.drain(..consumed);
                bytes_consumed += consumed as u64;
                count += 1;
            }
            None => {
                if eof {
                    break;
                }
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    eof = true;
                } else {
                    leftover.extend_from_slice(&buf[..n]);
                }
            }
        }

        if last_report.elapsed().as_secs() >= 15 {
            let pct = bytes_consumed as f64 / file_len as f64 * 100.0;
            let elapsed = start.elapsed().as_secs_f64();
            let eta = if pct > 0.0 {
                elapsed / pct * (100.0 - pct)
            } else {
                0.0
            };
            println!("  {:.1}% - {} records - ETA {:.0}s", pct, count, eta);
            last_report = std::time::Instant::now();
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!("\n=== RESULT ===");
    println!("  Total records:        {}", count);
    println!("  Bytes consumed:       {}", bytes_consumed);
    println!(
        "  Leftover bytes:       {} (should be 0 for valid file)",
        leftover.len()
    );
    println!("  Elapsed:              {:.1}s", elapsed);

    if !leftover.is_empty() {
        println!(
            "  ⚠️  WARNING: {} leftover bytes at EOF — file may be truncated!",
            leftover.len()
        );
        let preview: Vec<String> = leftover
            .iter()
            .take(20)
            .map(|b| format!("{:02x}", b))
            .collect();
        println!("  First leftover bytes: {}", preview.join(" "));
    } else {
        println!("  ✅ File ends on a clean record boundary");
    }

    Ok(())
}
