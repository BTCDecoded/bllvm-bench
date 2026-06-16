//! Find the first OutputRef record in a file that fails round-trip serialization.
//! Usage: diagnose_output_integrity [path] [max_records]

use blvm_bench::sort_merge::output_refs::OutputRef;
use std::fs::File;
use std::io::{BufReader, Read};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        "/mnt/extra/blockchain/sort_merge_data/outputs_unsorted.bin".to_string()
    });
    let max_records: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(u64::MAX);

    let file_len = std::fs::metadata(&path)?.len();
    println!(
        "Diagnosing: {} ({:.2} GB)",
        path,
        file_len as f64 / 1_073_741_824.0
    );

    let file = File::open(&path)?;
    let mut reader = BufReader::with_capacity(64 * 1024 * 1024, file);
    let mut buf = vec![0u8; 512 * 1024];
    let mut leftover: Vec<u8> = Vec::with_capacity(1024 * 1024);
    let mut count = 0u64;
    let mut file_offset = 0u64;
    let mut eof = false;

    loop {
        match OutputRef::from_bytes(&leftover) {
            Some((rec, consumed)) => {
                let orig = &leftover[..consumed];
                let declared_len =
                    u32::from_le_bytes([orig[49], orig[50], orig[51], orig[52]]) as usize;
                let roundtrip = rec.to_bytes();
                if roundtrip != orig {
                    println!("\n=== FIRST ROUND-TRIP MISMATCH ===");
                    println!("  record_index:     {}", count);
                    println!("  file_offset:      {}", file_offset);
                    println!("  block_height:     {}", rec.block_height);
                    println!("  output_idx:       {}", rec.output_idx);
                    println!("  declared_len:     {}", declared_len);
                    println!("  actual_script:    {} bytes", rec.script_pubkey.len());
                    println!("  encoded_on_disk:  {} bytes", consumed);
                    println!("  roundtrip_encode: {} bytes", roundtrip.len());
                    return Ok(());
                }
                if rec.script_pubkey.len() != declared_len {
                    println!("\n=== SCRIPT LENGTH FIELD MISMATCH ===");
                    println!("  record_index:     {}", count);
                    println!("  file_offset:      {}", file_offset);
                    println!("  block_height:     {}", rec.block_height);
                    println!("  declared_len:     {}", declared_len);
                    println!("  actual_script:    {} bytes", rec.script_pubkey.len());
                    return Ok(());
                }
                leftover.drain(..consumed);
                file_offset += consumed as u64;
                count += 1;
                if count % 100_000_000 == 0 {
                    println!(
                        "  scanned {} records ({:.1} GB)...",
                        count,
                        file_offset as f64 / 1_073_741_824.0
                    );
                }
                if count >= max_records {
                    println!("Stopped after {} records (limit); no mismatch found", count);
                    return Ok(());
                }
            }
            None => {
                if eof {
                    println!("\n=== EOF ===");
                    println!("  records_parsed: {}", count);
                    println!("  leftover_bytes: {}", leftover.len());
                    if !leftover.is_empty() {
                        let preview: String = leftover
                            .iter()
                            .take(32)
                            .map(|b| format!("{:02x}", b))
                            .collect();
                        println!("  leftover_head:  {}", preview);
                    }
                    return Ok(());
                }
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    eof = true;
                } else {
                    leftover.extend_from_slice(&buf[..n]);
                }
            }
        }
    }
}
